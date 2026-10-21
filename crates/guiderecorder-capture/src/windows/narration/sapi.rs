//! Dictation with SAPI 5's desktop recogniser, on the device.
//!
//! - The recognisers are the desktop ones Windows installs with a
//!   language's speech pack (`HKLM\SOFTWARE\Microsoft\Speech\Recognizers`,
//!   attributes `Desktop` and `Dictation`). [`recognizers`] lists them and
//!   [`choose`] picks the one for the user's language. None at all: speech
//!   can't be recognised here, and nothing is sent anywhere instead.
//! - [`Recogniser`] runs one in process (`SpInprocRecognizer`) with the
//!   dictation grammar, on a thread of its own. It reads 16 kHz 16-bit
//!   mono PCM from a [`Feed`] through a stream of ours ([`FeedStream`])
//!   whose reads wait until enough audio has come, so it hears the
//!   microphone live. Closing the feed ends the stream: the recogniser
//!   finishes the last phrase and says the stream ended.
//! - Each recognition is a phrase. Its words are timed in bytes of the
//!   stream ([`SPPHRASE`]'s position plus each element's offset), which
//!   [`BYTES_PER_SEC`] makes seconds of the audio fed.
//! - Dictation punctuates only what is spoken ("comma", "full stop"); a
//!   punctuation mark that takes no space before it is attached to the
//!   word before, as the Speech framework does on macOS.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{implement, w, Interface, Ref, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, E_NOTIMPL, STG_E_INVALIDFUNCTION, S_OK};
use windows::Win32::Media::Audio::WAVEFORMATEX;
use windows::Win32::Media::Speech::{
    ISpObjectToken, ISpPhrase, ISpRecoContext, ISpRecoGrammar, ISpRecoResult, ISpRecognizer,
    ISpStream, SpInprocRecognizer, SpObjectToken, SpStream, SPAF_CONSUME_LEADING_SPACES,
    SPEI_END_SR_STREAM, SPEI_RECOGNITION, SPEI_RESERVED1, SPEI_RESERVED2, SPET_LPARAM_IS_OBJECT,
    SPET_LPARAM_IS_POINTER, SPET_LPARAM_IS_STRING, SPET_LPARAM_IS_TOKEN, SPEVENT, SPLO_STATIC,
    SPRS_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, ISequentialStream_Impl,
    IStream, IStream_Impl, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, LOCKTYPE, STATFLAG, STATSTG,
    STGC, STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_SET,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};

use crate::narration::NarrationError;

/// What the recogniser is fed: 16 kHz, 16-bit, mono.
pub const RATE: u32 = 16_000;
pub const BYTES_PER_SEC: f64 = (RATE * 2) as f64;
/// `SPDFID_WaveFormatEx`: the stream's format is the `WAVEFORMATEX` given.
const SPDFID_WAVE_FORMAT_EX: windows::core::GUID =
    windows::core::GUID::from_u128(0xc31adbae_527f_4ff5_a230_f62bb61ff70c);
/// Where the recognisers are registered.
const TOKENS: PCWSTR = w!("SOFTWARE\\Microsoft\\Speech\\Recognizers\\Tokens");
/// How often the recognition thread looks for a cancel.
const POLL: Duration = Duration::from_millis(100);
/// How long starting may take (loading the recogniser's models).
const START_WAIT: Duration = Duration::from_secs(15);
/// How long a cancelled recogniser has to end before it is left.
const CANCEL_WAIT: Duration = Duration::from_secs(2);

/// A desktop dictation recogniser that is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Its key under [`TOKENS`], such as `MS-2057-80-DESK`.
    pub key: String,
    /// The languages it is for (`Language`): LANGIDs, its own first.
    pub languages: Vec<u16>,
    /// The other locales it takes (`SupportedLocales`).
    pub supported: Vec<u16>,
}

impl Token {
    /// The full token id SAPI takes.
    fn id(&self) -> String {
        format!(
            "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech\\Recognizers\\Tokens\\{}",
            self.key
        )
    }
}

/// A `;`-separated list of hex LANGIDs, as token attributes write them.
fn langids(list: &str) -> Vec<u16> {
    list.split(';')
        .filter_map(|id| u16::from_str_radix(id.trim(), 16).ok())
        .collect()
}

/// The installed desktop recognisers that can dictate, by key.
pub fn recognizers() -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut key = HKEY::default();
    // SAFETY: opens a key for reading into `key`, closed below.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, TOKENS, None, KEY_READ, &mut key) }
        != ERROR_SUCCESS
    {
        return tokens;
    }
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `name` holds `len` characters; the other outputs are
        // optional and not asked for.
        let found = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if found != ERROR_SUCCESS {
            break;
        }
        let name = String::from_utf16_lossy(&name[..len as usize]);
        let attributes = format!("{name}\\Attributes");
        let attribute = |value: &str| string_value(key, &attributes, value);
        if attribute("Desktop").is_none() || attribute("Dictation").is_none() {
            continue;
        }
        tokens.push(Token {
            languages: langids(&attribute("Language").unwrap_or_default()),
            supported: langids(&attribute("SupportedLocales").unwrap_or_default()),
            key: name,
        });
    }
    // SAFETY: the key opened above.
    let _ = unsafe { RegCloseKey(key) };
    tokens.sort_by(|a, b| a.key.cmp(&b.key));
    tokens
}

/// A string value under `root\subkey`; none when it is not there.
pub(crate) fn string_value(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let (subkey, value) = (HSTRING::from(subkey), HSTRING::from(value));
    let mut buf = [0u16; 512];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: `buf` holds `size` bytes; the type is restricted to REG_SZ,
    // which RegGetValueW NUL-terminates.
    let read = unsafe {
        RegGetValueW(
            root,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if read != ERROR_SUCCESS {
        return None;
    }
    let chars = (size as usize / 2).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..chars]);
    Some(text.trim_end_matches('\0').to_owned())
}

/// The recogniser the user picked in Speech settings, if any: its key.
fn default_key() -> Option<String> {
    let id = string_value(
        HKEY_CURRENT_USER,
        "SOFTWARE\\Microsoft\\Speech\\Recognizers",
        "DefaultTokenId",
    )?;
    id.rsplit('\\').next().map(str::to_owned)
}

/// The recogniser for a user whose language is `ui` (a LANGID): one for
/// that language; else one that takes it; else one for another variety of
/// it (en-US for en-GB); else the default one, or any. None when none is
/// installed.
pub fn choose<'a>(tokens: &'a [Token], ui: u16, default: Option<&str>) -> Option<&'a Token> {
    let primary = |id: u16| id & 0x3ff;
    tokens
        .iter()
        .find(|t| t.languages.contains(&ui))
        .or_else(|| tokens.iter().find(|t| t.supported.contains(&ui)))
        .or_else(|| {
            tokens
                .iter()
                .find(|t| t.languages.iter().any(|&l| primary(l) == primary(ui)))
        })
        .or_else(|| tokens.iter().find(|t| Some(t.key.as_str()) == default))
        .or_else(|| tokens.first())
}

/// The recogniser for the user's language, or why there is none (named by
/// its locale, such as `de-DE`).
pub fn for_user() -> Result<Token, NarrationError> {
    let ui = user_language();
    let tokens = recognizers();
    choose(&tokens, ui, default_key().as_deref())
        .cloned()
        .ok_or_else(|| NarrationError::NotOnDevice(locale_name(ui)))
}

/// The user's display language.
fn user_language() -> u16 {
    // SAFETY: no preconditions.
    unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() }
}

/// A LANGID's locale name, such as `en-GB`; its number if it has none.
fn locale_name(langid: u16) -> String {
    let mut buf = [0u16; 85];
    // SAFETY: `buf` is the documented LOCALE_NAME_MAX_LENGTH; a LANGID is
    // an LCID with the default sort.
    let len = unsafe {
        windows::Win32::Globalization::LCIDToLocaleName(u32::from(langid), Some(&mut buf), 0)
    };
    match usize::try_from(len) {
        Ok(len) if len > 1 => String::from_utf16_lossy(&buf[..len - 1]),
        _ => format!("language {langid:04x}"),
    }
}

/// Audio waiting for the recogniser: what [`FeedStream`] reads.
#[derive(Debug, Default)]
pub struct Feed {
    state: Mutex<FeedState>,
    /// Signalled when audio comes or the feed closes.
    changed: Condvar,
}

#[derive(Debug, Default)]
struct FeedState {
    bytes: VecDeque<u8>,
    closed: bool,
    /// Bytes read so far: the stream's position.
    read: u64,
}

impl Feed {
    fn lock(&self) -> MutexGuard<'_, FeedState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues 16 kHz mono samples. Ignored once closed.
    pub fn push(&self, samples: &[i16]) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        state
            .bytes
            .extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        self.changed.notify_all();
    }

    /// No more audio: reads take what is left, then the stream ends.
    pub fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    /// Waits until `want` bytes are queued or the feed closes, then takes
    /// up to `want`.
    fn take(&self, want: usize) -> Vec<u8> {
        let mut state = self.lock();
        while state.bytes.len() < want && !state.closed {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let n = want.min(state.bytes.len());
        state.read += n as u64;
        state.bytes.drain(..n).collect()
    }
}

/// A read-only COM stream over a [`Feed`], for `ISpStream::SetBaseStream`.
/// It can only say where it is, not seek.
#[implement(IStream)]
struct FeedStream(Arc<Feed>);

impl ISequentialStream_Impl for FeedStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        let bytes = self.0.take(cb as usize);
        // SAFETY: the caller gives `cb` writable bytes at `pv`, and `bytes`
        // is no longer than that; `pcbread` may be null.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pv.cast::<u8>(), bytes.len());
            if !pcbread.is_null() {
                *pcbread = bytes.len() as u32;
            }
        }
        S_OK
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        E_NOTIMPL
    }
}

impl IStream_Impl for FeedStream_Impl {
    fn Seek(
        &self,
        move_by: i64,
        origin: STREAM_SEEK,
        position: *mut u64,
    ) -> windows::core::Result<()> {
        let at = self.0.lock().read;
        // Only "where are we": no move from here, or to here.
        let stays = match origin {
            STREAM_SEEK_CUR => move_by == 0,
            STREAM_SEEK_SET => u64::try_from(move_by) == Ok(at),
            _ => false,
        };
        if !stays {
            return Err(STG_E_INVALIDFUNCTION.into());
        }
        if !position.is_null() {
            // SAFETY: a non-null out-pointer from the caller.
            unsafe { *position = at };
        }
        Ok(())
    }

    fn SetSize(&self, _size: u64) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn CopyTo(
        &self,
        _to: Ref<IStream>,
        _cb: u64,
        _read: *mut u64,
        _written: *mut u64,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _flags: &STGC) -> windows::core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn LockRegion(&self, _offset: u64, _cb: u64, _kind: &LOCKTYPE) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn UnlockRegion(&self, _offset: u64, _cb: u64, _kind: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Stat(&self, stat: *mut STATSTG, _flags: &STATFLAG) -> windows::core::Result<()> {
        if stat.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }
        // A live stream has no size yet: as long as it may get.
        // SAFETY: a non-null out-pointer from the caller.
        unsafe {
            *stat = STATSTG {
                r#type: STGTY_STREAM.0 as u32,
                cbSize: u64::MAX,
                ..Default::default()
            };
        }
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

/// A word heard: (seconds into the audio fed, duration, text).
pub type Heard = (f64, f64, String);

/// Dictation running on its own thread, fed through a [`Feed`].
pub struct Recogniser {
    feed: Arc<Feed>,
    words: Arc<Mutex<Vec<Heard>>>,
    cancel: Arc<AtomicBool>,
    /// Sent to when recognition ends (the stream ran out, or it failed).
    done: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Recogniser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recogniser").finish_non_exhaustive()
    }
}

impl Recogniser {
    /// Starts dictation with `token`; returns once it is listening, or
    /// with why it can't.
    pub fn start(token: &Token) -> Result<Self, NarrationError> {
        let feed = Arc::new(Feed::default());
        let words = Arc::new(Mutex::new(Vec::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let thread = {
            let (id, feed, words, cancel) =
                (token.id(), feed.clone(), words.clone(), cancel.clone());
            std::thread::Builder::new()
                .name("guiderecorder-dictation".into())
                .spawn(move || {
                    run(&id, &feed, &words, &cancel, ready_tx);
                    let _ = done_tx.send(());
                })
                .map_err(|e| NarrationError::Failed(e.to_string()))?
        };
        let recogniser = Self {
            feed,
            words,
            cancel,
            done,
            thread: Some(thread),
        };
        match ready.recv_timeout(START_WAIT) {
            Ok(Ok(())) => Ok(recogniser),
            Ok(Err(e)) => Err(e),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(NarrationError::Failed(
                "speech recognition did not start in time".into(),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(NarrationError::Failed("speech recognition stopped".into()))
            }
        }
    }

    /// Where its audio goes.
    pub fn feed(&self) -> &Arc<Feed> {
        &self.feed
    }

    /// Ends the audio and waits up to `wait` for the last phrase; returns
    /// every word heard, in the order heard.
    pub fn finish(mut self, wait: Duration) -> Vec<Heard> {
        self.feed.close();
        self.halt(wait);
        std::mem::take(&mut *self.words.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Waits up to `wait` for recognition to end by itself, then cancels
    /// it. A thread that still does not end (stuck in SAPI) is left to
    /// itself rather than waited on, so stopping never hangs.
    fn halt(&mut self, wait: Duration) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let ended = |done: &mpsc::Receiver<()>, wait| {
            !matches!(
                done.recv_timeout(wait),
                Err(mpsc::RecvTimeoutError::Timeout)
            )
        };
        let mut over = ended(&self.done, wait);
        self.cancel.store(true, Ordering::Relaxed);
        self.feed.close();
        if !over {
            over = ended(&self.done, CANCEL_WAIT);
        }
        if over {
            let _ = thread.join();
        } else {
            eprintln!("guiderecorder: narration: speech recognition did not stop; left running");
        }
    }
}

impl Drop for Recogniser {
    /// Cancels at once: what was heard is discarded.
    fn drop(&mut self) {
        self.halt(Duration::ZERO);
    }
}

/// The recognition thread: sets up SAPI, says whether that worked, then
/// collects words until the stream ends or it is cancelled.
fn run(
    token: &str,
    feed: &Arc<Feed>,
    words: &Mutex<Vec<Heard>>,
    cancel: &AtomicBool,
    ready: mpsc::Sender<Result<(), NarrationError>>,
) {
    // SAFETY: a thread of our own joins the multithreaded apartment, left
    // at its end below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if let Err(e) = com.ok() {
        let _ = ready.send(Err(NarrationError::Failed(e.message())));
        return;
    }
    {
        // SAFETY: SAPI objects made and used on this thread only.
        let sapi = match unsafe { Sapi::new(token, feed) } {
            Ok(sapi) => sapi,
            Err(e) => {
                let _ = ready.send(Err(NarrationError::Failed(format!(
                    "speech recognition could not start: {}",
                    e.message()
                ))));
                // SAFETY: pairs the CoInitializeEx above.
                unsafe { CoUninitialize() };
                return;
            }
        };
        let _ = ready.send(Ok(()));
        while !cancel.load(Ordering::Relaxed) {
            // SAFETY: live SAPI objects on their own thread.
            match unsafe { sapi.next(words) } {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    eprintln!(
                        "guiderecorder: narration: dictation failed: {}",
                        e.message()
                    );
                    break;
                }
            }
        }
    }
    // SAFETY: pairs the CoInitializeEx above, after every SAPI object of
    // this thread is released.
    unsafe { CoUninitialize() };
}

/// The SAPI objects dictation needs, kept alive together.
struct Sapi {
    _recognizer: ISpRecognizer,
    context: ISpRecoContext,
    _grammar: ISpRecoGrammar,
    _stream: ISpStream,
}

impl Sapi {
    /// An in-process recogniser for `token`, dictating from `feed`.
    unsafe fn new(token: &str, feed: &Arc<Feed>) -> windows::core::Result<Self> {
        // SAFETY (whole body): SAPI's documented set-up order (engine,
        // input, context, events, grammar) with objects this thread owns;
        // the format struct outlives the call that copies it.
        unsafe {
            let recognizer: ISpRecognizer =
                CoCreateInstance(&SpInprocRecognizer, None, CLSCTX_INPROC_SERVER)?;
            let engine: ISpObjectToken =
                CoCreateInstance(&SpObjectToken, None, CLSCTX_INPROC_SERVER)?;
            engine.SetId(PCWSTR::null(), &HSTRING::from(token), false)?;
            recognizer.SetRecognizer(&engine)?;

            let stream: ISpStream = CoCreateInstance(&SpStream, None, CLSCTX_INPROC_SERVER)?;
            let base: IStream = FeedStream(feed.clone()).into();
            let format = wave_format();
            stream.SetBaseStream(&base, &SPDFID_WAVE_FORMAT_EX, &format)?;
            recognizer.SetInput(&stream, false)?;

            let context = recognizer.CreateRecoContext()?;
            context.SetNotifyWin32Event()?;
            context.SetInterest(interest(), interest())?;
            let grammar = context.CreateGrammar(0)?;
            grammar.LoadDictation(PCWSTR::null(), SPLO_STATIC)?;
            grammar.SetDictationState(SPRS_ACTIVE)?;
            Ok(Self {
                _recognizer: recognizer,
                context,
                _grammar: grammar,
                _stream: stream,
            })
        }
    }

    /// Waits a little for events and takes them: false once the stream
    /// has ended.
    unsafe fn next(&self, words: &Mutex<Vec<Heard>>) -> windows::core::Result<bool> {
        // SAFETY (whole body): the context is live; every event fetched is
        // cleared once read.
        unsafe {
            // A timeout is not an error worth more than waiting again.
            let _ = self.context.WaitForNotifyEvent(POLL.as_millis() as u32);
            loop {
                let mut event = SPEVENT::default();
                let mut fetched = 0;
                self.context.GetEvents(1, &mut event, &mut fetched)?;
                if fetched == 0 {
                    return Ok(true);
                }
                let id = event._bitfield & 0xffff;
                let mut ended = false;
                if id == SPEI_RECOGNITION.0 {
                    if let Some(result) = event_object::<ISpRecoResult>(&event) {
                        let heard = phrase_words(&result);
                        words
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend(heard);
                    }
                } else if id == SPEI_END_SR_STREAM.0 {
                    ended = true;
                }
                clear(&mut event);
                if ended {
                    return Ok(false);
                }
            }
        }
    }
}

/// The events wanted: recognitions and the end of the stream
/// (`SPFEI(SPEI_RECOGNITION) | SPFEI(SPEI_END_SR_STREAM)`).
fn interest() -> u64 {
    let flag = |id: i32| 1u64 << id;
    flag(SPEI_RECOGNITION.0)
        | flag(SPEI_END_SR_STREAM.0)
        | flag(SPEI_RESERVED1.0)
        | flag(SPEI_RESERVED2.0)
}

/// The feed's format.
fn wave_format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: windows::Win32::Media::Audio::WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

/// The event's object (a recognition's result), borrowed.
unsafe fn event_object<T: Interface>(event: &SPEVENT) -> Option<T> {
    if (event._bitfield >> 16) != SPET_LPARAM_IS_OBJECT.0 || event.lParam.0 == 0 {
        return None;
    }
    // SAFETY: an object event's lParam is a live IUnknown the event holds
    // a reference to; `from_raw_borrowed` takes no reference of its own and
    // the cast adds one.
    unsafe { windows::core::IUnknown::from_raw_borrowed(&(event.lParam.0 as *mut _)) }
        .and_then(|unknown| unknown.cast().ok())
}

/// Releases what an event holds (`SpClearEvent`).
unsafe fn clear(event: &mut SPEVENT) {
    let kind = event._bitfield >> 16;
    let ptr = event.lParam.0 as *mut core::ffi::c_void;
    if ptr.is_null() {
        return;
    }
    // SAFETY: the event owns one reference (an object or token) or one
    // CoTaskMem allocation (a pointer or string) in lParam, given up here.
    unsafe {
        if kind == SPET_LPARAM_IS_OBJECT.0 || kind == SPET_LPARAM_IS_TOKEN.0 {
            drop(windows::core::IUnknown::from_raw(ptr));
        } else if kind == SPET_LPARAM_IS_POINTER.0 || kind == SPET_LPARAM_IS_STRING.0 {
            CoTaskMemFree(Some(ptr));
        }
    }
    event.lParam.0 = 0;
}

/// The words of a recognised phrase, timed in the stream.
unsafe fn phrase_words(phrase: &ISpPhrase) -> Vec<Heard> {
    // SAFETY: GetPhrase gives a CoTaskMem block holding the phrase and
    // its elements, read here and freed at the end.
    unsafe {
        let Ok(block) = phrase.GetPhrase() else {
            return Vec::new();
        };
        if block.is_null() {
            return Vec::new();
        }
        let base = &(*block).Base;
        let start = base.ullAudioStreamPosition as f64;
        let rule = &base.Rule;
        let elements = if base.pElements.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(
                base.pElements.add(rule.ulFirstElement as usize),
                rule.ulCountOfElements as usize,
            )
        };
        let parts = elements.iter().map(|e| Part {
            secs: (start + f64::from(e.ulAudioStreamOffset)) / BYTES_PER_SEC,
            duration: f64::from(e.ulAudioSizeBytes) / BYTES_PER_SEC,
            text: text(e.pszDisplayText),
            joins: i32::from(e.bDisplayAttributes) & SPAF_CONSUME_LEADING_SPACES.0 != 0,
        });
        let words = join(parts);
        CoTaskMemFree(Some(block as *const _));
        words
    }
}

/// A NUL-terminated string from SAPI, or nothing.
unsafe fn text(s: PCWSTR) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: a valid NUL-terminated string while the phrase is held.
    unsafe { s.to_string() }.unwrap_or_default()
}

/// One element of a phrase, before punctuation joins the word before.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub secs: f64,
    pub duration: f64,
    pub text: String,
    /// It takes no space before it (punctuation such as "," or ".").
    pub joins: bool,
}

/// Words from a phrase's parts: blank ones dropped, and one that takes no
/// space before it attached to the word before (when there is one).
pub fn join(parts: impl IntoIterator<Item = Part>) -> Vec<Heard> {
    let mut words: Vec<Heard> = Vec::new();
    for part in parts {
        let text = part.text.trim();
        if text.is_empty() {
            continue;
        }
        match words.last_mut() {
            Some(last) if part.joins => {
                last.2.push_str(text);
                last.1 = (part.secs + part.duration - last.0).max(last.1);
            }
            _ => words.push((part.secs, part.duration, text.to_owned())),
        }
    }
    words
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn token(key: &str, languages: &[u16], supported: &[u16]) -> Token {
        Token {
            key: key.into(),
            languages: languages.to_vec(),
            supported: supported.to_vec(),
        }
    }

    #[test]
    fn attribute_lists_read_as_langids() {
        assert_eq!(langids("809"), vec![0x809]);
        assert_eq!(langids("409;9; C09 ;x"), vec![0x409, 0x9, 0xc09]);
        assert!(langids("").is_empty());
    }

    #[test]
    fn the_recogniser_follows_the_users_language() {
        let uk = token("MS-2057-80-DESK", &[0x809], &[0x809, 0xc09, 0x9]);
        let us = token("MS-1033-80-DESK", &[0x409], &[0x409]);
        let de = token("MS-1031-80-DESK", &[0x407], &[0x407]);
        let all = [de.clone(), uk.clone(), us.clone()];
        // Its own language first.
        assert_eq!(choose(&all, 0x809, None), Some(&uk));
        assert_eq!(choose(&all, 0x409, None), Some(&us));
        // Australian English: the UK one takes it.
        assert_eq!(choose(&all, 0xc09, None), Some(&uk));
        // Canadian English: another English one.
        let only_us = [de.clone(), us.clone()];
        assert_eq!(choose(&only_us, 0x1009, None), Some(&us));
        // French: the default one, else the first.
        assert_eq!(choose(&all, 0x40c, Some("MS-1033-80-DESK")), Some(&us));
        assert_eq!(choose(&all, 0x40c, None), Some(&de));
        // None installed.
        assert_eq!(choose(&[], 0x809, None), None);
    }

    #[test]
    fn punctuation_joins_the_word_before() {
        let part = |secs: f64, text: &str, joins: bool| Part {
            secs,
            duration: 0.25,
            text: text.into(),
            joins,
        };
        let words = join([
            part(1.0, "open", false),
            part(1.5, "settings", false),
            part(2.0, ",", true),
            part(2.5, " ", false),
            part(3.0, "then", false),
            part(3.5, ".", true),
        ]);
        assert_eq!(
            words,
            vec![
                (1.0, 0.25, "open".into()),
                (1.5, 0.75, "settings,".into()),
                (3.0, 0.75, "then.".into()),
            ]
        );
        // Nothing before it: it stands alone.
        assert_eq!(join([part(0.0, ".", true)]), vec![(0.0, 0.25, ".".into())]);
    }

    #[test]
    fn the_feed_waits_for_enough_audio_then_ends() {
        let feed = Arc::new(Feed::default());
        let reader = {
            let feed = feed.clone();
            std::thread::spawn(move || (feed.take(4), feed.take(4), feed.take(4)))
        };
        feed.push(&[1]);
        std::thread::sleep(Duration::from_millis(20));
        feed.push(&[2, 3]);
        feed.close();
        // Ignored once closed.
        feed.push(&[4]);
        let (a, b, c) = reader.join().unwrap();
        assert_eq!(a, vec![1, 0, 2, 0]);
        assert_eq!(b, vec![3, 0]);
        assert!(c.is_empty());
        assert_eq!(feed.lock().read, 6);
    }

    #[test]
    fn the_feed_stream_reads_and_says_where_it_is() {
        let feed = Arc::new(Feed::default());
        feed.push(&[7, 8]);
        feed.close();
        let stream: IStream = FeedStream(feed).into();
        let mut buf = [0u8; 8];
        let mut read = 0;
        let mut at = 0;
        // SAFETY: a live stream and buffers of the sizes given.
        unsafe {
            stream
                .Read(buf.as_mut_ptr().cast(), 8, Some(&mut read))
                .ok()
                .unwrap();
            stream.Seek(0, STREAM_SEEK_CUR, Some(&mut at)).unwrap();
            assert!(stream
                .Seek(1, windows::Win32::System::Com::STREAM_SEEK_END, None)
                .is_err());
        }
        assert_eq!((read, at), (4, 4));
        assert_eq!(&buf[..4], &[7, 0, 8, 0]);
    }

    /// Speaks SAPI XML with the default desktop voice into a 16 kHz mono
    /// WAV at `path`; none when there is no voice.
    fn speak_to_wav(xml: &str, path: &std::path::Path) -> Option<()> {
        use windows::Win32::Media::Speech::{ISpVoice, SpVoice, SPFM_CREATE_ALWAYS, SPF_IS_XML};
        // SAFETY: SAPI objects used on this thread only; the format struct
        // outlives the call that copies it.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let voice: ISpVoice = CoCreateInstance(&SpVoice, None, CLSCTX_INPROC_SERVER).ok()?;
            voice.GetVoice().ok()?;
            let file: ISpStream = CoCreateInstance(&SpStream, None, CLSCTX_INPROC_SERVER).ok()?;
            let format = wave_format();
            file.BindToFile(
                &HSTRING::from(path),
                SPFM_CREATE_ALWAYS,
                Some(&SPDFID_WAVE_FORMAT_EX),
                Some(&format),
                0,
            )
            .ok()?;
            voice.SetOutput(&file, false).ok()?;
            voice
                .Speak(&HSTRING::from(xml), SPF_IS_XML.0 as u32, None)
                .ok()?;
            voice.WaitUntilDone(60_000).ok()?;
            file.Close().ok()?;
        }
        Some(())
    }

    /// The 16-bit samples of a PCM WAV's `data` chunk.
    pub(crate) fn wav_samples(bytes: &[u8]) -> Vec<i16> {
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
            if id == b"data" {
                return body
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&s| i16::from_le_bytes(s))
                    .collect();
            }
            at += 8 + len + (len & 1);
        }
        Vec::new()
    }

    /// Speech synthesised by SAPI goes through the live path (a feed read
    /// as it fills) and comes back as words, timed where they were said.
    #[test]
    fn synthesised_speech_is_recognised_with_its_timing() {
        let tokens = recognizers();
        // An English recogniser, as the voices on a test machine are.
        let Some(token) = tokens
            .iter()
            .find(|t| t.languages.iter().any(|l| l & 0x3ff == 0x09))
        else {
            eprintln!("skipped: no English desktop recogniser is installed");
            return;
        };
        let dir = std::env::temp_dir().join(format!("gr-dictation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("speech.wav");
        let lead = 2.0;
        let xml = format!(
            "<silence msec=\"{}\"/>Open the settings window and click the save button.\
             <silence msec=\"1500\"/>Then close the report.<silence msec=\"1000\"/>",
            (lead * 1000.0) as u32
        );
        if speak_to_wav(&xml, &wav).is_none() {
            eprintln!("skipped: no desktop voice is installed");
            return;
        }
        let samples = wav_samples(&std::fs::read(&wav).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
        let secs = samples.len() as f64 / f64::from(RATE);
        assert!(secs > lead + 2.0, "{secs} s of speech");

        let recogniser = Recogniser::start(token).unwrap();
        // In tenths of a second, as the microphone gives it.
        for chunk in samples.chunks(RATE as usize / 10) {
            recogniser.feed().push(chunk);
        }
        let started = std::time::Instant::now();
        let words = recogniser.finish(Duration::from_secs(20));
        assert!(started.elapsed() < Duration::from_secs(20));
        eprintln!("heard: {words:?}");

        let text: Vec<String> = words
            .iter()
            .map(|(_, _, w)| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .collect();
        let expected = [
            "open", "settings", "window", "click", "save", "button", "close", "report",
        ];
        let found = expected
            .iter()
            .filter(|e| text.iter().any(|t| t == *e))
            .count();
        assert!(found >= 5, "found {found} of {expected:?} in {text:?}");
        // In order, inside the audio, after the leading silence.
        for pair in words.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "{pair:?}");
        }
        for (start, duration, word) in &words {
            assert!(*duration > 0.0 && *duration < 3.0, "{word}: {duration}");
            assert!(*start >= lead - 0.3, "{word} at {start}, before the speech");
            assert!(start + duration <= secs + 0.05, "{word} past the end");
        }
        // The second sentence comes after the 1.5 s pause.
        let times = |of: &[&str]| -> Vec<f64> {
            text.iter()
                .zip(&words)
                .filter(|(t, _)| of.contains(&t.as_str()))
                .map(|(_, w)| w.0)
                .collect()
        };
        let first = times(&["open", "settings", "window", "click", "save", "button"]);
        let second = times(&["close", "report"]);
        if let (Some(end), Some(start)) = (
            first.iter().copied().reduce(f64::max),
            second.iter().copied().reduce(f64::min),
        ) {
            assert!(
                start - end > 1.5,
                "first sentence until {end}, second from {start}"
            );
        }
    }
}
