//! espeak-ng, the grapheme-to-phoneme engine behind Kokoro's tokenizer, through its C API
//! (`libespeak-ng`, the `libespeak-ng-dev` package). The library is process-global and not
//! thread-safe, so there is one instance, behind a lock.
//!
//! `phonemizer` (which `kokoro-onnx` calls) asks for IPA with `_` between phonemes and a
//! space between words (`espeak_TextToPhonemes` mode `'_' << 8 | espeakPHONEMES_IPA`), and
//! picks the voice by language code the way its `EspeakWrapper.set_voice` does: the first
//! voice listed for the code that isn't an mbrola one.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use crate::{InferError, Result};

#[repr(C)]
struct EspeakVoice {
    name: *const c_char,
    languages: *const c_char,
    identifier: *const c_char,
    gender: u8,
    age: u8,
    variant: u8,
    xx1: u8,
    score: c_int,
    spare: *mut c_void,
}

#[link(name = "espeak-ng")]
unsafe extern "C" {
    fn espeak_Initialize(
        output: c_int,
        buflength: c_int,
        path: *const c_char,
        options: c_int,
    ) -> c_int;
    fn espeak_SetVoiceByName(name: *const c_char) -> c_int;
    fn espeak_TextToPhonemes(
        textptr: *mut *const c_void,
        textmode: c_int,
        phonememode: c_int,
    ) -> *const c_char;
    fn espeak_ListVoices(voice_spec: *mut EspeakVoice) -> *const *const EspeakVoice;
    fn espeak_Info(path_data: *mut *const c_char) -> *const c_char;
}

const AUDIO_OUTPUT_SYNCHRONOUS: c_int = 0x02;
const CHARS_UTF8: c_int = 1;
const PHONEMES_IPA: c_int = 0x02;

/// The initialized library.
pub struct Espeak {
    lock: Mutex<Option<String>>,
    version: String,
    data_path: PathBuf,
}

static INSTANCE: OnceLock<std::result::Result<Espeak, String>> = OnceLock::new();

impl Espeak {
    /// Initialize once per process. `data_path` is the `espeak-ng-data` folder; `None`
    /// takes espeak-ng's own (`ESPEAK_DATA_PATH`, then the build's default, which the
    /// Ubuntu package puts under `/usr/lib/x86_64-linux-gnu/espeak-ng-data`). A second
    /// call with a different path returns the first instance.
    pub fn init(data_path: Option<&Path>) -> Result<&'static Espeak> {
        INSTANCE
            .get_or_init(|| Self::initialize(data_path))
            .as_ref()
            .map_err(|e| InferError::Espeak(e.clone()))
    }

    fn initialize(data_path: Option<&Path>) -> std::result::Result<Espeak, String> {
        let path = match data_path {
            Some(p) => {
                Some(CString::new(p.to_string_lossy().into_owned()).map_err(|e| e.to_string())?)
            }
            None => None,
        };
        let rate = unsafe {
            espeak_Initialize(
                AUDIO_OUTPUT_SYNCHRONOUS,
                0,
                path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                0,
            )
        };
        if rate <= 0 {
            return Err(format!(
                "failed to initialize espeak shared library{}",
                data_path
                    .map(|p| format!(" with data at {}", p.display()))
                    .unwrap_or_default()
            ));
        }
        let mut data: *const c_char = std::ptr::null();
        let version = unsafe { espeak_Info(&mut data) };
        let version = if version.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(version) }
                .to_string_lossy()
                .into_owned()
        };
        // espeak-1.48 appends the release date to the version number; phonemizer drops it.
        let version = version.split(' ').next().unwrap_or("").replace("-dev", "");
        let data_path = if data.is_null() {
            PathBuf::new()
        } else {
            PathBuf::from(
                unsafe { CStr::from_ptr(data) }
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        Ok(Espeak {
            lock: Mutex::new(None),
            version,
            data_path,
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// The data folder in use, as the library reports it.
    pub fn data_path(&self) -> &Path {
        &self.data_path
    }

    /// The voice identifier for a language code, as `EspeakWrapper.set_voice` picks it:
    /// the first voice whose first language is `code`, mbrola voices skipped.
    fn identifier_for(code: &str) -> Result<String> {
        let voices = unsafe { espeak_ListVoices(std::ptr::null_mut()) };
        if voices.is_null() {
            return Err(InferError::Espeak(
                "espeak_ListVoices returned nothing".into(),
            ));
        }
        let mut i = 0;
        loop {
            let voice = unsafe { *voices.add(i) };
            if voice.is_null() {
                break;
            }
            let voice = unsafe { &*voice };
            let identifier = unsafe { CStr::from_ptr(voice.identifier) }
                .to_string_lossy()
                .replace('\\', "/");
            if identifier.starts_with("mb/") {
                i += 1;
                continue;
            }
            // Pairs of (byte) priority + language; the first pair is the C string's text
            // after its first byte.
            let languages = unsafe { CStr::from_ptr(voice.languages) }.to_bytes();
            let language = String::from_utf8_lossy(languages.get(1..).unwrap_or(&[]));
            if language == code {
                return Ok(identifier);
            }
            i += 1;
        }
        Err(InferError::Espeak(format!("invalid voice code \"{code}\"")))
    }

    /// Phonemes for `text` in the voice for `language` ("en-us"): IPA, `_` between
    /// phonemes, a space between words, each clause espeak returns joined by a space
    /// (`EspeakWrapper.text_to_phonemes`).
    pub fn text_to_phonemes(&self, text: &str, language: &str) -> Result<String> {
        let mut voice = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        if voice.as_deref() != Some(language) {
            let identifier = Self::identifier_for(language)?;
            let name =
                CString::new(identifier.clone()).map_err(|e| InferError::Espeak(e.to_string()))?;
            if unsafe { espeak_SetVoiceByName(name.as_ptr()) } != 0 {
                return Err(InferError::Espeak(format!(
                    "failed to load voice \"{language}\""
                )));
            }
            *voice = Some(language.to_string());
        }
        let text = CString::new(text).map_err(|e| InferError::Espeak(e.to_string()))?;
        let mut ptr: *const c_void = text.as_ptr().cast();
        let mode = ((b'_' as c_int) << 8) | PHONEMES_IPA;
        let mut parts: Vec<String> = Vec::new();
        while !ptr.is_null() {
            let out = unsafe { espeak_TextToPhonemes(&mut ptr, CHARS_UTF8, mode) };
            if out.is_null() {
                continue;
            }
            let s = unsafe { CStr::from_ptr(out) }.to_string_lossy();
            if !s.is_empty() {
                parts.push(s.into_owned());
            }
        }
        Ok(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phonemizes_english_with_stress_and_underscores() {
        let espeak = Espeak::init(None).expect("libespeak-ng with its data");
        assert!(!espeak.version().is_empty());
        let out = espeak.text_to_phonemes("hello world", "en-us").unwrap();
        assert!(out.contains('_'), "{out}");
        assert!(out.contains("ˈ"), "{out}");
        assert_eq!(out.split(' ').count(), 2, "{out}");
        let again = espeak.text_to_phonemes("hello world", "en-us").unwrap();
        assert_eq!(out, again);
    }

    #[test]
    fn an_unknown_language_is_an_error() {
        let espeak = Espeak::init(None).expect("libespeak-ng with its data");
        assert!(espeak.text_to_phonemes("x", "zz-nowhere").is_err());
    }
}
