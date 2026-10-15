//! A deterministic in-process [`AiProvider`] for tests and offline
//! development. By default it echoes inputs with an `AI: ` prefix; the
//! `on_*` builders replace any response, including with errors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::{
    prompts, AiError, AiFuture, AiProvider, GenerationMeta, GuideInput, GuideOutput, RewriteInput,
    RewriteOutput, StepInput, StepOutput, Usage,
};

pub const MOCK_MODEL: &str = "mock";

type Handler<I, O> = Box<dyn Fn(&I) -> Result<O, AiError> + Send + Sync>;

#[derive(Default)]
pub struct MockProvider {
    on_step: Option<Handler<StepInput, StepOutput>>,
    on_guide: Option<Handler<GuideInput, GuideOutput>>,
    on_rewrite: Option<Handler<RewriteInput, RewriteOutput>>,
    step_calls: AtomicUsize,
    guide_calls: AtomicUsize,
    rewrite_calls: AtomicUsize,
    seen_steps: Mutex<Vec<StepInput>>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_step(
        mut self,
        f: impl Fn(&StepInput) -> Result<StepOutput, AiError> + Send + Sync + 'static,
    ) -> Self {
        self.on_step = Some(Box::new(f));
        self
    }

    pub fn on_guide(
        mut self,
        f: impl Fn(&GuideInput) -> Result<GuideOutput, AiError> + Send + Sync + 'static,
    ) -> Self {
        self.on_guide = Some(Box::new(f));
        self
    }

    pub fn on_rewrite(
        mut self,
        f: impl Fn(&RewriteInput) -> Result<RewriteOutput, AiError> + Send + Sync + 'static,
    ) -> Self {
        self.on_rewrite = Some(Box::new(f));
        self
    }

    pub fn step_calls(&self) -> usize {
        self.step_calls.load(Ordering::SeqCst)
    }

    pub fn guide_calls(&self) -> usize {
        self.guide_calls.load(Ordering::SeqCst)
    }

    pub fn rewrite_calls(&self) -> usize {
        self.rewrite_calls.load(Ordering::SeqCst)
    }

    /// Step inputs received so far, in call order.
    pub fn seen_steps(&self) -> Vec<StepInput> {
        self.seen_steps.lock().unwrap().clone()
    }
}

/// Metadata the mock attaches to its outputs.
pub fn mock_meta(prompt_version: &str) -> GenerationMeta {
    GenerationMeta {
        model: MOCK_MODEL.into(),
        prompt_version: prompt_version.into(),
        usage: Usage::default(),
    }
}

impl AiProvider for MockProvider {
    fn generate_step<'a>(&'a self, input: &'a StepInput) -> AiFuture<'a, StepOutput> {
        self.step_calls.fetch_add(1, Ordering::SeqCst);
        self.seen_steps.lock().unwrap().push(input.clone());
        let result = match &self.on_step {
            Some(f) => f(input),
            None => Ok(StepOutput {
                title: format!("AI: {}", input.template_title),
                description: None,
                redaction_suggestions: Vec::new(),
                meta: mock_meta(prompts::STEP.version),
            }),
        };
        Box::pin(async move { result })
    }

    fn generate_guide<'a>(&'a self, input: &'a GuideInput) -> AiFuture<'a, GuideOutput> {
        self.guide_calls.fetch_add(1, Ordering::SeqCst);
        let result = match &self.on_guide {
            Some(f) => f(input),
            None => Ok(GuideOutput {
                title: format!("AI: {}", input.current_title),
                intro: String::new(),
                prerequisites: String::new(),
                summary: format!("{} steps", input.steps.len()),
                tags: input.apps.iter().map(|app| app.to_lowercase()).collect(),
                step_edits: Vec::new(),
                meta: mock_meta(prompts::GUIDE.version),
            }),
        };
        Box::pin(async move { result })
    }

    fn rewrite<'a>(&'a self, input: &'a RewriteInput) -> AiFuture<'a, RewriteOutput> {
        self.rewrite_calls.fetch_add(1, Ordering::SeqCst);
        let result = match &self.on_rewrite {
            Some(f) => f(input),
            None => Ok(RewriteOutput {
                title: format!("AI: {}", input.title),
                description: (!input.description.is_empty()).then(|| input.description.clone()),
                meta: mock_meta(prompts::REWRITE.version),
            }),
        };
        Box::pin(async move { result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::concurrency::generate_steps;
    use crate::model::{Audience, StepKind};
    use std::sync::Arc;
    use ulid::Ulid;

    fn step(title: &str) -> StepInput {
        StepInput {
            step_id: Ulid::new(),
            kind: StepKind::Click,
            template_title: title.into(),
            template_description: String::new(),
            element: None,
            keystrokes: None,
            crop: None,
            full: None,
            previous_title: None,
            next_title: None,
            guide_title: None,
            language: "en".into(),
            audience: Audience::Beginner,
        }
    }

    #[tokio::test]
    async fn default_responses() {
        let mock = MockProvider::new();
        let out = mock.generate_step(&step("Click OK")).await.unwrap();
        assert_eq!(out.title, "AI: Click OK");
        assert_eq!(out.meta.prompt_version, "step.v1");
        assert_eq!(mock.step_calls(), 1);
        assert_eq!(mock.seen_steps()[0].template_title, "Click OK");
    }

    #[tokio::test]
    async fn drives_bounded_step_pass_through_dyn_provider() {
        let mock = Arc::new(MockProvider::new().on_step(|input| {
            if input.template_title == "fail" {
                Err(AiError::Refusal { category: None })
            } else {
                Ok(StepOutput {
                    title: input.template_title.to_uppercase(),
                    description: None,
                    redaction_suggestions: Vec::new(),
                    meta: mock_meta("step.v1"),
                })
            }
        }));
        let provider: Arc<dyn AiProvider> = mock.clone();
        let inputs = vec![step("a"), step("fail"), step("c")];
        let mut results = Vec::new();
        generate_steps(provider, inputs, 2, |i, r| {
            results.push((i, r.map(|o| o.title)))
        })
        .await;
        results.sort_by_key(|(i, _)| *i);
        assert_eq!(
            results,
            [
                (0, Ok("A".to_string())),
                (1, Err(AiError::Refusal { category: None })),
                (2, Ok("C".to_string())),
            ]
        );
        assert_eq!(mock.step_calls(), 3);
    }
}
