//! espeak-ng, the grapheme-to-phoneme engine behind Kokoro's tokenizer, through its C API
//! (`libespeak-ng`, the `libespeak-ng-dev` package). The library is process-global and not
//! thread-safe, so there is one instance, behind a lock.
//!
//! `phonemizer` (which `kokoro-onnx` calls) asks for IPA with `_` between phonemes and a
//! space between words (`espeak_TextToPhonemes` mode `'_' << 8 | espeakPHONEMES_IPA`), and
//! picks the voice by language code the way its `EspeakWrapper.set_voice` does: the first
//! voice listed for the code that isn't an mbrola one.
//!
//! The library needs its data folder (`espeak-ng-data`), made for its own version, and
//! looks for it at the path its build chose: Ubuntu's library at
//! `/usr/lib/x86_64-linux-gnu/espeak-ng-data`, where Arch and Fedora keep theirs at
//! `/usr/share/espeak-ng-data` (#168). So Ghost names the folder itself ([`choose`]):
//!
//! 1. `GHOST_ESPEAK_DATA`, when set: the operator's or a test's own folder, taken as given.
//! 2. The bundle's own copy beside the running executable (`<bin>/../lib/heka-ghost-desktop/
//!    espeak-ng-data`, the AppImage's, which carries Ubuntu's library), when the version it
//!    was staged for ([`STAMP`], written by `scripts/espeak.sh`) is the loaded library's.
//! 3. The system's folders, `/usr/lib/<arch>-linux-gnu/espeak-ng-data` (Debian, Ubuntu)
//!    then `/usr/share/espeak-ng-data` (Arch, Fedora), when the loaded library is the
//!    system's own: the .deb's `ghostd` and a checkout's link the system library, whose
//!    package brings the data that matches it.
//! 4. Otherwise the library's own default.
//!
//! A folder that does not fit is a plain error naming the library's version and the
//! folder, never espeak's own `exit(1)`: `espeak_Initialize` ends the process when its
//! data is missing, so Ghost initializes through `espeak_ng_Initialize`, which returns.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use crate::runtime::{non_empty, resources_beside};
use crate::{InferError, Result};

/// The operator's own data folder, honoured first.
pub const ENV_VAR: &str = "GHOST_ESPEAK_DATA";
/// The data folder's name, in the bundle's resources and on the system.
pub const FOLDER: &str = "espeak-ng-data";
/// In the bundle's copy: the espeak-ng version it was staged for (`1.51`), then where it
/// came from. Written by `scripts/espeak.sh`; a system folder has none.
pub const STAMP: &str = "GHOST-ESPEAK-NG-VERSION";

/// The system's data folders, in the order they are tried.
pub fn system_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from(format!(
            "/usr/lib/{}-linux-gnu/{FOLDER}",
            std::env::consts::ARCH
        )),
        PathBuf::from(format!("/usr/share/{FOLDER}")),
    ]
}

/// The bundle's copy, for the executable at `exe`: `<dir of exe>/../lib/heka-ghost-desktop/
/// espeak-ng-data` (`runtime::resources_beside`).
pub fn bundle_dir(exe: &Path) -> Option<PathBuf> {
    Some(resources_beside(exe.parent()?)?.join(FOLDER))
}

/// The library that is loaded: its version and, when the process can tell, its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    /// As `espeak_Info` says it, without a date or `-dev` (`1.51`, `1.52.0`).
    pub version: String,
    pub path: Option<PathBuf>,
}

impl Library {
    /// The system's own library: one under `/usr/lib`, `/usr/lib64` or `/lib`, not the
    /// copy a bundle carries. Unknown counts as the system's (a checkout's build).
    pub fn is_system(&self) -> bool {
        self.path.as_deref().is_none_or(|p| {
            [
                "/usr/lib/",
                "/usr/lib64/",
                "/lib/",
                "/lib64/",
                "/usr/local/lib/",
            ]
            .iter()
            .any(|d| p.starts_with(d))
        })
    }

    fn named(&self) -> String {
        match &self.path {
            Some(p) => format!("espeak-ng {} ({})", self.version, p.display()),
            None => format!("espeak-ng {}", self.version),
        }
    }
}

/// `1.51` from `1.51`, `1.51.0`, `1.51+dfsg-12build1`: the release, which is what a data
/// folder is made for.
fn release(version: &str) -> String {
    let v = version.trim();
    let v = v.split(['+', '-', ' ', '~']).next().unwrap_or(v);
    v.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// The version a folder's stamp names, when it has one.
pub fn stamped_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(STAMP)).ok()?;
    let first = text.lines().next()?.trim();
    (!first.is_empty()).then(|| first.to_string())
}

/// A folder espeak-ng can read: its phoneme table is there.
pub fn looks_like_data(dir: &Path) -> bool {
    dir.join("phontab").is_file() && dir.join("phondata").is_file()
}

/// What was wrong with espeak-ng's data, in a closed set (a report's fact, ADR 0067).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataProblem {
    /// No data folder where it was looked for, or a file of it missing.
    Missing,
    /// A folder made for another version of espeak-ng than the library loaded.
    VersionMismatch,
    /// A folder espeak-ng could not read for another reason.
    Unreadable,
}

impl DataProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "data_missing",
            Self::VersionMismatch => "data_version_mismatch",
            Self::Unreadable => "data_unreadable",
        }
    }
}

fn data_error(kind: DataProblem, message: String) -> InferError {
    InferError::EspeakData { kind, message }
}

/// Where the data came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Setting,
    Bundle,
    System,
    /// The library's own default (`ESPEAK_DATA_PATH`, then its build's path).
    Default,
}

/// The folder to initialize with, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub dir: Option<PathBuf>,
    pub source: Source,
}

/// The data folder for `library`, by the order above. `explicit` is the setting
/// (`GHOST_ESPEAK_DATA` or a caller's own), `bundle` the bundle's copy, `system` the
/// system's folders in order. Pure over the files under those paths.
pub fn choose(
    explicit: Option<&Path>,
    bundle: Option<&Path>,
    system: &[PathBuf],
    library: &Library,
) -> std::result::Result<Choice, (DataProblem, String)> {
    let wanted = release(&library.version);
    if let Some(dir) = explicit {
        if !looks_like_data(dir) {
            return Err((
                DataProblem::Missing,
                format!(
                    "{ENV_VAR} names {}, which is not an espeak-ng-data folder (it has no phontab)",
                    dir.display()
                ),
            ));
        }
        if let Some(stamp) = stamped_version(dir)
            && release(&stamp) != wanted
        {
            return Err((
                DataProblem::VersionMismatch,
                format!("{ENV_VAR}: {}", mismatch(library, dir, &stamp)),
            ));
        }
        return Ok(Choice {
            dir: Some(dir.to_path_buf()),
            source: Source::Setting,
        });
    }
    let mut bundled_other = None;
    if let Some(dir) = bundle.filter(|d| looks_like_data(d)) {
        match stamped_version(dir) {
            Some(stamp) if release(&stamp) == wanted => {
                return Ok(Choice {
                    dir: Some(dir.to_path_buf()),
                    source: Source::Bundle,
                });
            }
            Some(stamp) => bundled_other = Some((dir.to_path_buf(), stamp)),
            None => {}
        }
    }
    let on_system = system.iter().find(|d| looks_like_data(d));
    if library.is_system() {
        if let Some(dir) = on_system {
            return Ok(Choice {
                dir: Some(dir.clone()),
                source: Source::System,
            });
        }
        return Ok(Choice {
            dir: None,
            source: Source::Default,
        });
    }
    // A bundled library with no data of its own version here: the system's folder is for
    // the system's espeak-ng, not this one.
    if let Some((dir, stamp)) = bundled_other {
        return Err((
            DataProblem::VersionMismatch,
            mismatch(library, &dir, &stamp),
        ));
    }
    Err(match on_system {
        Some(dir) => (
            DataProblem::VersionMismatch,
            format!(
                "{} has no data of its own here, and {} is the system's, made for the system's \
             espeak-ng; set {ENV_VAR} to an espeak-ng-data folder for {}",
                library.named(),
                dir.display(),
                library.version
            ),
        ),
        None => (
            DataProblem::Missing,
            format!(
                "{} has no data folder here (looked in {}); set {ENV_VAR} to an espeak-ng-data \
             folder for {}",
                library.named(),
                bundle
                    .into_iter()
                    .map(Path::to_path_buf)
                    .chain(system.iter().cloned())
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                library.version
            ),
        ),
    })
}

fn mismatch(library: &Library, dir: &Path, stamp: &str) -> String {
    format!(
        "the data at {} is for espeak-ng {}, but the library loaded is {}",
        dir.display(),
        stamp,
        library.named()
    )
}

/// The file the loaded `libespeak-ng` came from, read from `/proc/self/maps`.
fn loaded_library_path() -> Option<PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines()
        .filter_map(|l| l.split_whitespace().nth(5))
        .find(|p| {
            Path::new(p)
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("libespeak-ng.so"))
        })
        .map(PathBuf::from)
}

/// The loaded library, before it is initialized.
pub fn library() -> Library {
    // espeak_Info only hands back its static version string (and the data path when
    // asked); it reads nothing before initialization.
    let version = unsafe { espeak_Info(std::ptr::null_mut()) };
    let version = if version.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(version) }
            .to_string_lossy()
            .into_owned()
    };
    Library {
        version: clean_version(&version),
        path: loaded_library_path(),
    }
}

// espeak-1.48 appends the release date to the version number; phonemizer drops it.
fn clean_version(version: &str) -> String {
    version.split(' ').next().unwrap_or("").replace("-dev", "")
}

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
    fn espeak_ng_InitializePath(path: *const c_char);
    fn espeak_ng_Initialize(context: *mut *mut c_void) -> u32;
    fn espeak_ng_InitializeOutput(mode: c_int, buffer_length: c_int, device: *const c_char) -> u32;
    fn espeak_ng_ClearErrorContext(context: *mut *mut c_void);
    fn espeak_ng_GetStatusCodeMessage(status: u32, buffer: *mut c_char, length: usize);
    fn espeak_SetVoiceByName(name: *const c_char) -> c_int;
    fn espeak_TextToPhonemes(
        textptr: *mut *const c_void,
        textmode: c_int,
        phonememode: c_int,
    ) -> *const c_char;
    fn espeak_ListVoices(voice_spec: *mut EspeakVoice) -> *const *const EspeakVoice;
    fn espeak_Info(path_data: *mut *const c_char) -> *const c_char;
}

/// `ENOUTPUT_MODE_SYNCHRONOUS`, what `espeak_Initialize(AUDIO_OUTPUT_SYNCHRONOUS, ..)` sets.
const OUTPUT_SYNCHRONOUS: c_int = 0x0001;
const ENS_OK: u32 = 0;
const ENS_VERSION_MISMATCH: u32 = 0x1000_02FF;
const CHARS_UTF8: c_int = 1;
const PHONEMES_IPA: c_int = 0x02;

/// The initialized library.
pub struct Espeak {
    lock: Mutex<Option<String>>,
    version: String,
    data_path: PathBuf,
    source: Source,
}

static INSTANCE: OnceLock<Espeak> = OnceLock::new();
/// One initialization at a time; a failed one may be tried again (the files may arrive).
static INIT: Mutex<()> = Mutex::new(());

impl Espeak {
    /// Initialize once per process. `data_path` is the `espeak-ng-data` folder to use as
    /// given; `None` chooses one by the order in this module's notes. A second call after
    /// a success returns the first instance whatever it is given; a failure is not kept, so
    /// a later call tries again.
    pub fn init(data_path: Option<&Path>) -> Result<&'static Espeak> {
        if let Some(e) = INSTANCE.get() {
            return Ok(e);
        }
        let _one = INIT.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(e) = INSTANCE.get() {
            return Ok(e);
        }
        let env = non_empty(std::env::var_os(ENV_VAR));
        let explicit = data_path.map(Path::to_path_buf).or(env);
        let exe = std::env::current_exe().ok();
        let bundle = exe.as_deref().and_then(bundle_dir);
        let library = library();
        let choice = choose(
            explicit.as_deref(),
            bundle.as_deref(),
            &system_dirs(),
            &library,
        )
        .map_err(|(kind, message)| data_error(kind, message))?;
        let espeak = Self::initialize(&choice, &library).map_err(|e| match e {
            // The operator's folder: said as theirs, so the advice is to fix the setting.
            InferError::EspeakData { kind, message } if choice.source == Source::Setting => {
                data_error(kind, format!("{ENV_VAR}: {message}"))
            }
            e => e,
        })?;
        Ok(INSTANCE.get_or_init(|| espeak))
    }

    fn initialize(choice: &Choice, library: &Library) -> Result<Espeak> {
        let path = match &choice.dir {
            Some(p) => Some(
                CString::new(p.to_string_lossy().into_owned())
                    .map_err(|e| data_error(DataProblem::Unreadable, e.to_string()))?,
            ),
            None => None,
        };
        let status = unsafe {
            espeak_ng_InitializePath(path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()));
            let mut context: *mut c_void = std::ptr::null_mut();
            let status = espeak_ng_Initialize(&mut context);
            espeak_ng_ClearErrorContext(&mut context);
            status
        };
        if status != ENS_OK {
            let mut buffer = [0 as c_char; 512];
            unsafe { espeak_ng_GetStatusCodeMessage(status, buffer.as_mut_ptr(), buffer.len()) };
            let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
                .to_string_lossy()
                .trim()
                .trim_end_matches('.')
                .to_string();
            let mut data: *const c_char = std::ptr::null();
            unsafe { espeak_Info(&mut data) };
            let at = choice.dir.clone().unwrap_or_else(|| {
                if data.is_null() {
                    PathBuf::from("its default folder")
                } else {
                    PathBuf::from(
                        unsafe { CStr::from_ptr(data) }
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
            });
            // espeak's errno codes are below its own group (0x10000000).
            const ENOENT: u32 = 2;
            return Err(if status == ENS_VERSION_MISMATCH {
                data_error(
                    DataProblem::VersionMismatch,
                    format!(
                        "the data at {} is for another version of espeak-ng than the library \
                     loaded, {}",
                        at.display(),
                        library.named()
                    ),
                )
            } else {
                let kind = if status == ENOENT {
                    DataProblem::Missing
                } else {
                    DataProblem::Unreadable
                };
                data_error(
                    kind,
                    format!(
                        "{} could not read its data at {} ({message}); set {ENV_VAR} to an \
                     espeak-ng-data folder for {}",
                        library.named(),
                        at.display(),
                        library.version
                    ),
                )
            });
        }
        unsafe { espeak_ng_InitializeOutput(OUTPUT_SYNCHRONOUS, 0, std::ptr::null()) };
        let mut data: *const c_char = std::ptr::null();
        unsafe { espeak_Info(&mut data) };
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
            version: library.version.clone(),
            data_path,
            source: choice.source,
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// The data folder in use, as the library reports it.
    pub fn data_path(&self) -> &Path {
        &self.data_path
    }

    /// Where the data folder came from.
    pub fn source(&self) -> Source {
        self.source
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

    /// A data folder espeak-ng would read, stamped for `stamp` when given.
    fn data(dir: &Path, stamp: Option<&str>) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("phontab"), b"x").unwrap();
        std::fs::write(dir.join("phondata"), b"x").unwrap();
        if let Some(v) = stamp {
            std::fs::write(dir.join(STAMP), format!("{v}\nfrom a test\n")).unwrap();
        }
        dir.to_path_buf()
    }

    fn bundled(version: &str) -> Library {
        Library {
            version: version.into(),
            path: Some("/tmp/.mount_ghostX/usr/lib/libespeak-ng.so.1".into()),
        }
    }

    fn system(version: &str) -> Library {
        Library {
            version: version.into(),
            path: Some("/usr/lib/x86_64-linux-gnu/libespeak-ng.so.1".into()),
        }
    }

    #[test]
    fn the_bundles_library_reads_the_bundles_data_where_the_system_keeps_its_own_elsewhere() {
        // #168: the AppImage's Ubuntu library on Arch, whose data is at /usr/share.
        let t = tempfile::tempdir().unwrap();
        let bundle = data(&t.path().join("bundle/espeak-ng-data"), Some("1.51"));
        let arch = data(&t.path().join("usr/share/espeak-ng-data"), None);
        let ubuntu_path = t.path().join("usr/lib/x86_64-linux-gnu/espeak-ng-data");
        let c = choose(None, Some(&bundle), &[ubuntu_path, arch], &bundled("1.51")).unwrap();
        assert_eq!(c.dir.as_deref(), Some(bundle.as_path()));
        assert_eq!(c.source, Source::Bundle);
    }

    #[test]
    fn the_systems_library_reads_the_systems_data_and_skips_a_bundle_copy_for_another_version() {
        // The .deb on Ubuntu 26.04: the system's 1.52 library; a 1.51 copy is not its.
        let t = tempfile::tempdir().unwrap();
        let bundle = data(&t.path().join("bundle/espeak-ng-data"), Some("1.51"));
        let debian = data(
            &t.path().join("usr/lib/x86_64-linux-gnu/espeak-ng-data"),
            None,
        );
        let c = choose(None, Some(&bundle), &[debian.clone()], &system("1.52.0")).unwrap();
        assert_eq!(c.dir.as_deref(), Some(debian.as_path()));
        assert_eq!(c.source, Source::System);
        // The first system folder that has the data, in order.
        let missing = t.path().join("nowhere");
        let c = choose(None, None, &[missing, debian.clone()], &system("1.52.0")).unwrap();
        assert_eq!(c.dir.as_deref(), Some(debian.as_path()));
        // None at all: the library's own default, which may still find one.
        let c = choose(None, None, &[], &system("1.52.0")).unwrap();
        assert_eq!((c.dir, c.source), (None, Source::Default));
    }

    #[test]
    fn the_setting_comes_first_and_a_stamp_for_another_version_is_said_with_both() {
        let t = tempfile::tempdir().unwrap();
        let own = data(&t.path().join("own"), None);
        let bundle = data(&t.path().join("bundle/espeak-ng-data"), Some("1.51"));
        let c = choose(Some(&own), Some(&bundle), &[], &bundled("1.51")).unwrap();
        assert_eq!(
            (c.dir.as_deref(), c.source),
            (Some(own.as_path()), Source::Setting)
        );

        let old = data(&t.path().join("old"), Some("1.50"));
        let e = choose(Some(&old), None, &[], &bundled("1.51"))
            .unwrap_err()
            .1;
        assert!(
            e.contains("is for espeak-ng 1.50") && e.contains("espeak-ng 1.51"),
            "{e}"
        );
        assert!(e.contains(&old.display().to_string()), "{e}");

        let e = choose(Some(&t.path().join("empty")), None, &[], &bundled("1.51"))
            .unwrap_err()
            .1;
        assert!(
            e.contains(ENV_VAR) && e.contains("not an espeak-ng-data folder"),
            "{e}"
        );
    }

    #[test]
    fn a_bundled_library_never_takes_the_systems_data_and_says_why() {
        let t = tempfile::tempdir().unwrap();
        let arch = data(&t.path().join("usr/share/espeak-ng-data"), None);
        // No bundle copy at all.
        let e = choose(None, None, &[arch.clone()], &bundled("1.51"))
            .unwrap_err()
            .1;
        assert!(e.contains("is the system's") && e.contains(ENV_VAR), "{e}");
        assert!(e.contains("espeak-ng 1.51"), "{e}");
        // A bundle copy for another version: that mismatch is the one said.
        let bundle = data(&t.path().join("bundle/espeak-ng-data"), Some("1.50"));
        let e = choose(None, Some(&bundle), &[arch], &bundled("1.51"))
            .unwrap_err()
            .1;
        assert!(
            e.contains("is for espeak-ng 1.50") && e.contains("espeak-ng 1.51"),
            "{e}"
        );
        // Nothing anywhere: where it looked.
        let e = choose(
            None,
            Some(&t.path().join("b")),
            &[t.path().join("s")],
            &bundled("1.51"),
        )
        .unwrap_err()
        .1;
        assert!(
            e.contains("looked in") && e.contains("/b") && e.contains("/s"),
            "{e}"
        );
    }

    #[test]
    fn versions_compare_by_release() {
        assert_eq!(release("1.51"), "1.51");
        assert_eq!(release("1.52.0"), "1.52");
        assert_eq!(release("1.51+dfsg-12build1"), "1.51");
        assert!(bundled("1.51").path.is_some() && !bundled("1.51").is_system());
        assert!(system("1.51").is_system());
        assert!(
            Library {
                version: "1.52".into(),
                path: Some("/usr/lib/libespeak-ng.so.1".into())
            }
            .is_system()
        );
        assert!(
            Library {
                version: "1.52".into(),
                path: None
            }
            .is_system()
        );
    }

    #[test]
    fn the_loaded_library_is_found_and_named() {
        let lib = library();
        assert!(!lib.version.is_empty());
        let path = lib.path.expect("libespeak-ng in /proc/self/maps");
        assert!(
            path.to_string_lossy().contains("libespeak-ng.so"),
            "{}",
            path.display()
        );
    }

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
