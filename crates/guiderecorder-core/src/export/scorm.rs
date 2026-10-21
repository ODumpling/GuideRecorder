//! SCORM 1.2 package writer over [`RenderGuide`]: the interactive
//! walkthrough as a course a learning management system (LMS) can track.
//!
//! # Package
//!
//! ```text
//! imsmanifest.xml   the course: metadata, one organization, one SCO
//! guide.html        the walkthrough, with the SCORM runtime and a quiz
//! ```
//!
//! [`manifest`] names the course ([`ScormOptions::title`], else the guide
//! title) and gives it an identifier ([`ScormOptions::identifier`], made
//! a valid XML id by [`course_identifier`], else `guiderecorder-<guide id>`,
//! which stays the same across exports so an LMS can tell a new version of
//! the course). Its one item points at one `sco` resource, `guide.html`,
//! which lists every file in the package. The XSD files are not included;
//! the manifest names them in `xsi:schemaLocation` as usual.
//!
//! # SCO
//!
//! `guide.html` is the walkthrough page
//! ([`walkthrough`](super::walkthrough)) plus [`SCRIPT`], a SCORM 1.2 API
//! wrapper allowed by its hash like the walkthrough's own script. It looks
//! for the LMS's `API` object in this window, its parents and its opener,
//! calls `LMSInitialize`, sets `cmi.core.lesson_status` (`incomplete`
//! first), bookmarks the slide shown in `cmi.core.lesson_location` (and
//! starts there next time), and on leaving sets the session time and
//! `cmi.core.exit`, commits and calls `LMSFinish`. Without an LMS it does
//! nothing, so the package also works opened on its own.
//!
//! # Completion and quiz
//!
//! With [`Completion::LastStep`] the lesson is `completed` once the last
//! step is shown. A short multiple-choice quiz ([`parse_quiz`]) can follow
//! on the end slide; its score (0 to 100, the share of right answers) is
//! reported as `cmi.core.score.raw` (min 0, max 100) and compared with the
//! mastery score. With [`Completion::QuizPassed`] (which needs a quiz) the
//! lesson is `passed` or `failed` by that comparison instead, and the
//! manifest carries `adlcp:masteryscore`. A pass is not undone by a worse
//! try.
//!
//! The quiz is given with the export (it is not stored in the guide), as
//! text: a line per question, then a Markdown task-list line per choice,
//! `- [x]` for the right one. The right answers are in the page, so a
//! learner who reads its source can find them; it is a check of
//! understanding, not an exam.
//!
//! # Determinism
//!
//! The same guide and options give the same bytes: zip entries carry the
//! zip crate's fixed 1980-01-01 date, and the page is the walkthrough's.

use std::fmt::Write as _;
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::html::{escape, HtmlMode, HtmlOptions, GUIDE_FILE};
use super::md::{self, CheckedBranding, Labels};
use super::walkthrough::{self, Extra};
use super::{persist, ExportReport, ImageOptions, RenderGuide};
use crate::library::GuideHandle;

/// File name of the manifest at the root of the package.
pub const MANIFEST_FILE: &str = "imsmanifest.xml";
/// File name of the SCO (the walkthrough page) in the package.
pub const SCO_FILE: &str = GUIDE_FILE;

/// The mastery score when none is given.
pub const DEFAULT_MASTERY_SCORE: u8 = 80;
/// Most questions a quiz may have.
pub const MAX_QUESTIONS: usize = 50;
/// Most choices a question may have.
pub const MAX_CHOICES: usize = 10;
/// Longest course identifier [`course_identifier`] keeps.
pub const MAX_IDENTIFIER_CHARS: usize = 100;

/// The SCORM runtime, run before the walkthrough's script.
pub const SCRIPT: &str = include_str!("templates/scorm.js");
const STYLE: &str = include_str!("templates/scorm.css");

/// The Content-Security-Policy of the SCO page: the walkthrough's, also
/// allowing [`SCRIPT`] by its hash.
pub fn csp() -> String {
    walkthrough::csp_with(Some(SCRIPT))
}

/// When the LMS counts the course as done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Completion {
    /// `completed` once the last step is shown.
    #[default]
    LastStep,
    /// `passed` or `failed` by the quiz score against the mastery score.
    QuizPassed,
}

impl Completion {
    /// As the page's `data-completion` names it.
    fn as_str(self) -> &'static str {
        match self {
            Self::LastStep => "last-step",
            Self::QuizPassed => "quiz-passed",
        }
    }
}

/// A multiple-choice question with one right answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuizQuestion {
    /// Plain text.
    pub question: String,
    /// Plain text, in the order shown.
    pub choices: Vec<String>,
    /// Index of the right choice.
    pub answer: usize,
}

/// Why [`parse_quiz`] refused a quiz.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("quiz line {line}: {message}")]
pub struct QuizError {
    /// 1-based.
    pub line: usize,
    pub message: String,
}

/// Options for [`export`] and [`render_page`].
#[derive(Debug, Clone, PartialEq)]
pub struct ScormOptions {
    /// How step images are rendered; see [`HtmlOptions::image`].
    pub image: ImageOptions,
    /// Replace an existing file at the output path.
    pub overwrite: bool,
    /// Fixed labels; `None` picks them from the guide language.
    pub labels: Option<Labels>,
    pub branding: CheckedBranding,
    /// Show the version, owner and last review day under the summary.
    pub review_details: bool,
    /// The course title in the LMS; `None` or blank: the guide title.
    pub title: Option<String>,
    /// The course identifier ([`course_identifier`]); `None` or nothing
    /// usable: `guiderecorder-<guide id>`.
    pub identifier: Option<String>,
    pub completion: Completion,
    /// The quiz score (0 to 100) that passes.
    pub mastery_score: u8,
    /// The quiz on the end slide; empty for none.
    pub quiz: Vec<QuizQuestion>,
}

impl Default for ScormOptions {
    fn default() -> Self {
        let html = HtmlOptions::default();
        Self {
            image: html.image,
            overwrite: false,
            labels: None,
            branding: CheckedBranding::default(),
            review_details: false,
            title: None,
            identifier: None,
            completion: Completion::default(),
            mastery_score: DEFAULT_MASTERY_SCORE,
            quiz: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScormError {
    #[error("{0} already exists")]
    Exists(PathBuf),
    /// The options can't make a course (e.g. a quiz rule without a quiz).
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Render(#[from] md::ExportError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("packaging failed: {0}")]
    Package(String),
}

pub type Result<T> = std::result::Result<T, ScormError>;

/// Parses a quiz: a line per question, each followed by its choices as
/// Markdown task-list lines (`- [ ] wrong`, `- [x] right`; `*` works too).
/// Blank lines are ignored. Every question needs two to [`MAX_CHOICES`]
/// choices and exactly one right one; at most [`MAX_QUESTIONS`] questions.
pub fn parse_quiz(text: &str) -> std::result::Result<Vec<QuizQuestion>, QuizError> {
    let fail = |line: usize, message: &str| QuizError {
        line,
        message: message.to_owned(),
    };
    // Each question with the line it is on and its right choices.
    let mut questions: Vec<(usize, QuizQuestion, usize)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((right, choice)) = parse_choice(line) {
            let Some((_, question, rights)) = questions.last_mut() else {
                return Err(fail(n, "a choice before the first question"));
            };
            if choice.is_empty() {
                return Err(fail(n, "an empty choice"));
            }
            if question.choices.len() == MAX_CHOICES {
                return Err(fail(
                    n,
                    &format!("a question can have at most {MAX_CHOICES} choices"),
                ));
            }
            if right {
                question.answer = question.choices.len();
                *rights += 1;
            }
            question.choices.push(choice.to_owned());
        } else {
            if questions.len() == MAX_QUESTIONS {
                return Err(fail(
                    n,
                    &format!("a quiz can have at most {MAX_QUESTIONS} questions"),
                ));
            }
            let question = QuizQuestion {
                question: line.to_owned(),
                choices: Vec::new(),
                answer: 0,
            };
            questions.push((n, question, 0));
        }
    }
    questions
        .into_iter()
        .map(|(n, question, rights)| {
            if question.choices.len() < 2 {
                Err(fail(
                    n,
                    "a question needs at least two choices (`- [ ] …` lines)",
                ))
            } else if rights != 1 {
                Err(fail(
                    n,
                    "a question needs exactly one right choice (`- [x] …`)",
                ))
            } else {
                Ok(question)
            }
        })
        .collect()
}

/// `- [x] text` as `(true, "text")`, `- [ ] text` as `(false, "text")`.
fn parse_choice(line: &str) -> Option<(bool, &str)> {
    let rest = line
        .strip_prefix("- [")
        .or_else(|| line.strip_prefix("* ["))?;
    let mut chars = rest.chars();
    let right = match chars.next()? {
        ' ' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    let rest = chars.as_str().strip_prefix(']')?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some((right, rest.trim()))
}

/// `raw` as an identifier the manifest can use (an XML `ID`): ASCII
/// letters, digits, `.`, `-` and `_`, other characters as `-`, starting
/// with a letter or `_` (else `course-` is put first), at most
/// [`MAX_IDENTIFIER_CHARS`]. `None` when nothing is left.
pub fn course_identifier(raw: &str) -> Option<String> {
    let mut id = String::new();
    for c in raw.trim().chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            c
        } else {
            '-'
        };
        // One `-` for a run of characters that can't be used.
        if !(c == '-' && id.ends_with('-')) {
            id.push(c);
        }
    }
    let id = id.trim_matches('-');
    if id.is_empty() {
        return None;
    }
    let mut id = if id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        id.to_owned()
    } else {
        format!("course-{id}")
    };
    id.truncate(MAX_IDENTIFIER_CHARS);
    Some(id)
}

/// The course identifier `opts` give for `guide`.
pub fn identifier(guide: &RenderGuide, opts: &ScormOptions) -> String {
    opts.identifier
        .as_deref()
        .and_then(course_identifier)
        .unwrap_or_else(|| format!("guiderecorder-{}", guide.id))
}

/// The course title `opts` give for `guide`.
pub fn title(guide: &RenderGuide, opts: &ScormOptions) -> String {
    [opts.title.as_deref(), Some(guide.title.as_str())]
        .into_iter()
        .flatten()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| "Guide".to_owned())
}

/// `imsmanifest.xml` of a course: SCORM 1.2 metadata, one organization
/// with one item, and the SCO resource listing every file of the package.
/// `identifier` must be a valid id ([`course_identifier`]); the other ids
/// are made from it. The mastery score is given only with
/// [`Completion::QuizPassed`], so an LMS doesn't judge a course by a score
/// it isn't meant to count.
pub fn manifest(identifier: &str, title: &str, completion: Completion, mastery: u8) -> String {
    let id = escape(identifier);
    let title = escape(title);
    let mastery = match completion {
        Completion::QuizPassed => {
            format!("\n        <adlcp:masteryscore>{mastery}</adlcp:masteryscore>")
        }
        Completion::LastStep => String::new(),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<manifest identifier="{id}" version="1.0"
    xmlns="http://www.imsproject.org/xsd/imscp_rootv1p1p2"
    xmlns:adlcp="http://www.adlnet.org/xsd/adlcp_rootv1p2"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xsi:schemaLocation="http://www.imsproject.org/xsd/imscp_rootv1p1p2 imscp_rootv1p1p2.xsd http://www.imsglobal.org/xsd/imsmd_rootv1p2p1 imsmd_rootv1p2p1.xsd http://www.adlnet.org/xsd/adlcp_rootv1p2 adlcp_rootv1p2.xsd">
  <metadata>
    <schema>ADL SCORM</schema>
    <schemaversion>1.2</schemaversion>
  </metadata>
  <organizations default="{id}-org">
    <organization identifier="{id}-org">
      <title>{title}</title>
      <item identifier="{id}-item" identifierref="{id}-sco" isvisible="true">
        <title>{title}</title>{mastery}
      </item>
    </organization>
  </organizations>
  <resources>
    <resource identifier="{id}-sco" type="webcontent" adlcp:scormtype="sco" href="{SCO_FILE}">
      <file href="{SCO_FILE}"/>
    </resource>
  </resources>
</manifest>
"#
    )
}

/// The quiz's own words, by guide language like [`Labels`]. `{score}` is
/// filled in by the page, `{mastery}` here.
struct Words {
    quiz: &'static str,
    check: &'static str,
    unanswered: &'static str,
    passed: &'static str,
    failed: &'static str,
}

impl Words {
    fn for_language(language: &str) -> Self {
        let primary = language.split(['-', '_']).next().unwrap_or_default();
        if primary.eq_ignore_ascii_case("de") {
            Self {
                quiz: "Quiz",
                check: "Antworten prüfen",
                unanswered: "Beantworten Sie alle Fragen und prüfen Sie dann erneut.",
                passed: "Bestanden: {score} % richtig.",
                failed: "Nicht bestanden: {score} % richtig, {mastery} % nötig. Ändern Sie \
                         Ihre Antworten und prüfen Sie erneut.",
            }
        } else {
            Self {
                quiz: "Quiz",
                check: "Check answers",
                unanswered: "Answer every question, then check again.",
                passed: "Passed: {score}% correct.",
                failed: "Not passed: {score}% correct, {mastery}% needed. Change your answers \
                         and check again.",
            }
        }
    }
}

/// The end slide's markup: the runtime's settings and the quiz, if any.
fn end_html(opts: &ScormOptions, words: &Words) -> String {
    let mut html = format!(
        r#"<div class="scorm" id="scorm" data-completion="{}" data-mastery="{}">"#,
        opts.completion.as_str(),
        opts.mastery_score
    );
    if !opts.quiz.is_empty() {
        let answers: Vec<String> = opts.quiz.iter().map(|q| q.answer.to_string()).collect();
        let mastery = opts.mastery_score.to_string();
        let _ = write!(
            html,
            "\n<form class=\"scorm-quiz\" id=\"scorm-quiz\" data-answers=\"{}\" \
             data-passed=\"{}\" data-failed=\"{}\" data-unanswered=\"{}\">\n<h3>{}</h3>",
            answers.join(" "),
            escape(words.passed),
            escape(&words.failed.replace("{mastery}", &mastery)),
            escape(words.unanswered),
            escape(words.quiz),
        );
        for (i, q) in opts.quiz.iter().enumerate() {
            let n = i + 1;
            let _ = write!(
                html,
                "\n<fieldset class=\"scorm-question\">\n<legend>{n}. {}</legend>",
                escape(&q.question)
            );
            for (c, choice) in q.choices.iter().enumerate() {
                let _ = write!(
                    html,
                    "\n<label><input type=\"radio\" name=\"scorm-q{n}\" value=\"{c}\"> {}</label>",
                    escape(choice)
                );
            }
            html.push_str("\n</fieldset>");
        }
        let _ = write!(
            html,
            "\n<p class=\"scorm-check\"><button type=\"submit\" class=\"wt-button wt-primary\">{}</button></p>\
             \n<p class=\"scorm-result\" role=\"status\" aria-live=\"polite\"></p>\n</form>",
            escape(words.check)
        );
    }
    html.push_str("\n</div>");
    html
}

/// Refuses options that can't make a course.
fn check(opts: &ScormOptions) -> Result<()> {
    if opts.mastery_score > 100 {
        return Err(ScormError::Invalid(
            "the mastery score must be from 0 to 100".to_owned(),
        ));
    }
    if opts.completion == Completion::QuizPassed && opts.quiz.is_empty() {
        return Err(ScormError::Invalid(
            "completion by passing the quiz needs a quiz".to_owned(),
        ));
    }
    if opts.quiz.len() > MAX_QUESTIONS {
        return Err(ScormError::Invalid(format!(
            "a quiz can have at most {MAX_QUESTIONS} questions"
        )));
    }
    for q in &opts.quiz {
        if q.choices.len() < 2 || q.choices.len() > MAX_CHOICES || q.answer >= q.choices.len() {
            return Err(ScormError::Invalid(format!(
                "quiz question {:?} needs 2 to {MAX_CHOICES} choices and a right one",
                q.question
            )));
        }
    }
    Ok(())
}

/// The SCO page (`guide.html`) as a string; the report has the image
/// warnings and no files. Also the Export dialog's preview.
pub fn render_page(
    guide: &RenderGuide,
    handle: &GuideHandle,
    opts: &ScormOptions,
) -> Result<(String, ExportReport)> {
    check(opts)?;
    let html_opts = HtmlOptions {
        mode: HtmlMode::Walkthrough,
        image: opts.image.clone(),
        overwrite: false,
        labels: opts.labels.clone(),
        branding: opts.branding.clone(),
        review_details: opts.review_details,
    };
    let end = end_html(opts, &Words::for_language(&guide.language));
    let extra = Extra {
        script: SCRIPT,
        style: STYLE,
        end: &end,
    };
    Ok(walkthrough::render_with(
        guide,
        handle,
        &html_opts,
        Some(&extra),
    )?)
}

/// Writes `guide` as a SCORM 1.2 zip to `out_path`, with step images read
/// from the bundle at `handle`. Refuses an existing file unless
/// `opts.overwrite`.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_path: impl AsRef<Path>,
    opts: &ScormOptions,
) -> Result<ExportReport> {
    let out = out_path.as_ref();
    let exists = |e: io::Error| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            ScormError::Exists(out.to_path_buf())
        } else {
            ScormError::Io(e)
        }
    };
    persist::check_free(out, opts.overwrite).map_err(exists)?;
    let (page, mut report) = render_page(guide, handle, opts)?;
    let manifest = manifest(
        &identifier(guide, opts),
        &title(guide, opts),
        opts.completion,
        opts.mastery_score,
    );
    let mut bytes = io::Cursor::new(Vec::new());
    package(&mut bytes, &manifest, &page).map_err(|e| ScormError::Package(e.to_string()))?;
    persist::write_file(out, opts.overwrite, "zip", |file| {
        file.write_all(bytes.get_ref())
    })
    .map_err(exists)?;
    report.files.push(out.to_path_buf());
    Ok(report)
}

/// The zip: the manifest first, then the SCO.
fn package<W: Write + Seek>(writer: W, manifest: &str, page: &str) -> zip::result::ZipResult<()> {
    let mut zip = ZipWriter::new(writer);
    let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(MANIFEST_FILE, deflate)?;
    zip.write_all(manifest.as_bytes())?;
    zip.start_file(SCO_FILE, deflate)?;
    zip.write_all(page.as_bytes())?;
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quizzes_parse() {
        let quiz = parse_quiz(
            "Which button creates a key?\n- [ ] Delete\n- [x] Create key\n\n  \
             Where is it?\n* [X] Settings\n* [ ]   API keys  \n",
        )
        .unwrap();
        assert_eq!(
            quiz,
            [
                QuizQuestion {
                    question: "Which button creates a key?".into(),
                    choices: vec!["Delete".into(), "Create key".into()],
                    answer: 1,
                },
                QuizQuestion {
                    question: "Where is it?".into(),
                    choices: vec!["Settings".into(), "API keys".into()],
                    answer: 0,
                },
            ]
        );
        assert_eq!(parse_quiz("").unwrap(), []);
        assert_eq!(parse_quiz(" \n\n").unwrap(), []);
        // Not a choice: a question.
        assert_eq!(parse_choice("- [y] no"), None);
        assert_eq!(parse_choice("- [x]no"), None);
        assert_eq!(parse_choice("- [x]"), Some((true, "")));
    }

    #[test]
    fn bad_quizzes_say_where() {
        let err = |text: &str| parse_quiz(text).unwrap_err();
        assert_eq!(err("- [x] A").line, 1);
        assert!(err("- [x] A").message.contains("before the first question"));
        let e = err("Q?\n- [x] A\n- [x] B");
        assert_eq!((e.line, e.message.contains("exactly one")), (1, true));
        let e = err("Q?\n- [x] A\n- [ ] B\nR?\n- [ ] A\n- [ ] B");
        assert_eq!((e.line, e.message.contains("exactly one")), (4, true));
        let e = err("Q?\n- [x] A\nR?");
        assert_eq!((e.line, e.message.contains("two choices")), (1, true));
        assert_eq!(err("Q?\n- [x]\n- [ ] B").line, 2);
        assert_eq!(
            err("Q?\n- [ ]  \n- [x] B").to_string(),
            "quiz line 2: an empty choice"
        );
    }

    #[test]
    fn identifiers_are_xml_ids() {
        assert_eq!(
            course_identifier("api-keys_2.0").as_deref(),
            Some("api-keys_2.0")
        );
        assert_eq!(
            course_identifier(" Rotate an API key! ").as_deref(),
            Some("Rotate-an-API-key")
        );
        assert_eq!(
            course_identifier("2024 intro").as_deref(),
            Some("course-2024-intro")
        );
        assert_eq!(course_identifier("_x").as_deref(), Some("_x"));
        assert_eq!(course_identifier("Ärger").as_deref(), Some("rger"));
        assert_eq!(course_identifier(" !! "), None);
        assert_eq!(
            course_identifier(&"a".repeat(300)).map(|id| id.len()),
            Some(MAX_IDENTIFIER_CHARS)
        );
    }

    #[test]
    fn the_mastery_score_is_only_given_for_the_quiz_rule() {
        let last = manifest("c", "T & <co>", Completion::LastStep, 80);
        assert!(!last.contains("masteryscore"));
        assert!(last.contains("<title>T &amp; &lt;co&gt;</title>"));
        let quiz = manifest("c", "T", Completion::QuizPassed, 75);
        assert!(quiz.contains("<adlcp:masteryscore>75</adlcp:masteryscore>"));
    }

    #[test]
    fn the_end_slide_carries_the_settings_and_quiz() {
        let words = Words::for_language("en");
        let opts = ScormOptions::default();
        assert_eq!(
            end_html(&opts, &words),
            "<div class=\"scorm\" id=\"scorm\" data-completion=\"last-step\" data-mastery=\"80\">\n</div>"
        );
        let opts = ScormOptions {
            completion: Completion::QuizPassed,
            mastery_score: 50,
            quiz: parse_quiz("Is <b> bold?\n- [ ] No\n- [x] \"Yes\"").unwrap(),
            ..ScormOptions::default()
        };
        let html = end_html(&opts, &words);
        assert!(html.contains("data-completion=\"quiz-passed\" data-mastery=\"50\""));
        assert!(html.contains("data-answers=\"1\""));
        assert!(html.contains("<legend>1. Is &lt;b&gt; bold?</legend>"));
        assert!(html.contains("value=\"1\"> &quot;Yes&quot;</label>"));
        assert!(html.contains("data-failed=\"Not passed: {score}% correct, 50% needed."));
        assert_eq!(Words::for_language("de-CH").check, "Antworten prüfen");
    }

    #[test]
    fn options_are_checked() {
        let quiz = parse_quiz("Q?\n- [x] A\n- [ ] B").unwrap();
        let bad = [
            ScormOptions {
                completion: Completion::QuizPassed,
                ..ScormOptions::default()
            },
            ScormOptions {
                mastery_score: 101,
                ..ScormOptions::default()
            },
            ScormOptions {
                quiz: vec![QuizQuestion {
                    answer: 2,
                    ..quiz[0].clone()
                }],
                ..ScormOptions::default()
            },
        ];
        for opts in bad {
            assert!(
                matches!(check(&opts), Err(ScormError::Invalid(_))),
                "{opts:?}"
            );
        }
        let good = ScormOptions {
            completion: Completion::QuizPassed,
            quiz,
            ..ScormOptions::default()
        };
        assert!(check(&good).is_ok());
    }

    #[test]
    fn the_script_can_be_inlined() {
        assert!(!SCRIPT.contains("</script"));
        assert!(!STYLE.contains("</style"));
    }
}
