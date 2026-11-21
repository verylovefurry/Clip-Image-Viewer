//! LibRaw in an isolated worker process.
//!
//! LibRaw is C++ code parsing untrusted camera files, so the viewer never calls it in its own
//! process. The viewer starts its own executable with [`WORKER_FLAG`]:
//! - with a duplicate of the user token lowered to **Low integrity**, so a compromised worker
//!   cannot write to the user's files, registry or other processes at medium integrity;
//! - assigned to a **Job Object** (before it runs) that kills it with the job, allows a single
//!   process, caps committed memory, dies on unhandled exceptions and blocks UI/clipboard access;
//! - with only two inherited handles (stdin/stdout pipes) and no file path: the RAW bytes arrive
//!   through stdin and the result leaves through stdout;
//! - under a wall-clock limit enforced by the viewer.
//!
//! This is not an AppContainer: Low integrity does not deny reading files the user can read,
//! and network access is not blocked. It is stricter than the Electron app, which ran LibRaw
//! inside its own process.

use std::io::{Read, Write};

use image::{DynamicImage, RgbImage};

use crate::{Result, error};

pub const WORKER_FLAG: &str = "--clipview-libraw-worker";
const MAGIC: &[u8; 4] = b"CLR1";
const MAX_INPUT: u64 = 512 << 20;
const MAX_RGB: u64 = 64 * 1024 * 1024 * 3;
const MAX_JPEG: u64 = 64 << 20;
const MAX_RAW_MB: u32 = 2048;
const MIN_PREVIEW_EDGE: u32 = 1024;
const HALF_SIZE_ABOVE: u64 = 24_000_000;
const EXIT_INPUT: i32 = 20;
const EXIT_DECODE: i32 = 21;
const EXIT_OUTPUT: i32 = 22;

#[repr(C)]
struct RawImage { kind: i32, width: i32, height: i32, flip: i32, size: u64, data: *const u8, owner: *mut std::ffi::c_void }

impl Default for RawImage {
    fn default() -> Self { Self { kind: 0, width: 0, height: 0, flip: 0, size: 0, data: std::ptr::null(), owner: std::ptr::null_mut() } }
}

impl Drop for RawImage {
    fn drop(&mut self) { unsafe { clipview_libraw_free(self) } }
}

impl RawImage {
    fn bytes(&self) -> &[u8] {
        if self.data.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(self.data, self.size as usize) } }
    }
}

unsafe extern "C" {
    fn clipview_libraw_thumbnail(data: *const u8, len: usize, max_raw_mb: u32, out: *mut RawImage) -> i32;
    fn clipview_libraw_develop(data: *const u8, len: usize, max_raw_mb: u32, half_size: i32, out: *mut RawImage) -> i32;
    fn clipview_libraw_raw_pixels(data: *const u8, len: usize, max_raw_mb: u32, pixels: *mut u64) -> i32;
    fn clipview_libraw_free(image: *mut RawImage);
}

/// LibRaw orientation to EXIF orientation.
fn exif_orientation(flip: i32) -> u32 {
    match flip { 3 => 3, 5 => 8, 6 => 6, _ => 1 }
}

fn emit(out: &mut impl Write, kind: u32, width: u32, height: u32, orientation: u32, data: &[u8]) -> std::io::Result<()> {
    let mut header = Vec::with_capacity(32);
    header.extend_from_slice(MAGIC);
    for value in [kind, width, height, orientation, 0] { header.extend_from_slice(&value.to_le_bytes()); }
    header.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.write_all(&header)?;
    out.write_all(data)?;
    out.flush()
}

/// Thumbnail when it is large enough for viewing, otherwise a developed image.
fn produce(input: &[u8]) -> std::result::Result<(u32, u32, u32, u32, Vec<u8>), i32> {
    let mut thumb = RawImage::default();
    let thumb_ok = unsafe { clipview_libraw_thumbnail(input.as_ptr(), input.len(), MAX_RAW_MB, &mut thumb) } == 0;
    let thumb_size = if !thumb_ok { None } else if thumb.kind == 1 {
        image::ImageReader::with_format(std::io::Cursor::new(thumb.bytes()), image::ImageFormat::Jpeg).into_dimensions().ok()
    } else { Some((thumb.width as u32, thumb.height as u32)) };
    let packed = |image: &RawImage, size: (u32, u32)| (image.kind as u32, size.0, size.1, exif_orientation(image.flip), image.bytes().to_vec());
    if let Some(size) = thumb_size && size.0.max(size.1) >= MIN_PREVIEW_EDGE { return Ok(packed(&thumb, size)); }
    let mut pixels = 0;
    let half = unsafe { clipview_libraw_raw_pixels(input.as_ptr(), input.len(), MAX_RAW_MB, &mut pixels) } == 0 && pixels > HALF_SIZE_ABOVE;
    let mut developed = RawImage::default();
    if unsafe { clipview_libraw_develop(input.as_ptr(), input.len(), MAX_RAW_MB, i32::from(half), &mut developed) } == 0 && developed.kind == 2 {
        let size = (developed.width as u32, developed.height as u32);
        return Ok(packed(&developed, size));
    }
    // A small embedded preview is still better than nothing.
    thumb_size.map(|size| packed(&thumb, size)).ok_or(EXIT_DECODE)
}

/// Entry point of the worker process. Never call after the app has initialized.
pub fn worker_main() -> i32 {
    let mut input = Vec::new();
    if std::io::stdin().lock().take(MAX_INPUT + 1).read_to_end(&mut input).is_err() || input.is_empty() || input.len() as u64 > MAX_INPUT {
        return EXIT_INPUT;
    }
    // Debug builds only: verifies the sandbox by trying to create the file named after the
    // marker. A Low integrity worker must be denied writing into the user's folders.
    #[cfg(debug_assertions)]
    if let Some(path) = input.strip_prefix(b"CLIPVIEW-PROBE-WRITE:") {
        let path = String::from_utf8_lossy(path).trim().to_owned();
        let verdict: &[u8] = if std::fs::File::create(&path).is_ok() { b"WRITTEN" } else { b"DENIED" };
        return if emit(&mut std::io::stdout().lock(), 3, 1, 1, 1, verdict).is_ok() { 0 } else { EXIT_OUTPUT };
    }
    match produce(&input) {
        Ok((kind, width, height, orientation, data)) => {
            let limit = if kind == 1 { MAX_JPEG } else { MAX_RGB };
            if data.len() as u64 > limit || (kind == 2 && data.len() as u64 != u64::from(width) * u64::from(height) * 3) { return EXIT_OUTPUT; }
            if emit(&mut std::io::stdout().lock(), kind, width, height, orientation, &data).is_err() { return EXIT_OUTPUT; }
            0
        }
        Err(code) => code,
    }
}

/// Result of a LibRaw worker run.
pub enum Output {
    /// Camera JPEG with the container orientation (the JPEG may carry its own EXIF orientation).
    Jpeg { data: Vec<u8>, width: u32, height: u32, orientation: Option<u8> },
    /// Developed or bitmap image, already upright.
    Image(DynamicImage),
}

fn parse(output: &[u8]) -> Result<Output> {
    let invalid = || "RAW 작업 프로세스 출력이 올바르지 않습니다.".to_owned();
    if output.len() < 32 || &output[..4] != MAGIC { return Err(invalid()); }
    let word = |at: usize| u32::from_le_bytes(output[at..at + 4].try_into().unwrap());
    let (kind, width, height, orientation) = (word(4), word(8), word(12), word(16));
    let len = u64::from_le_bytes(output[24..32].try_into().unwrap());
    let data = output.get(32..).filter(|d| d.len() as u64 == len).ok_or_else(invalid)?;
    crate::picture::check_dimensions(width, height)?;
    match kind {
        1 => Ok(Output::Jpeg { data: data.to_vec(), width, height, orientation: (orientation != 1).then_some(orientation as u8) }),
        2 => {
            let rgb = RgbImage::from_raw(width, height, data.to_vec()).ok_or_else(invalid)?;
            let mut image = DynamicImage::ImageRgb8(rgb);
            if let Some(o) = image::metadata::Orientation::from_exif(orientation as u8) { image.apply_orientation(o); }
            Ok(Output::Image(image))
        }
        _ => Err(invalid()),
    }
}

/// Decodes RAW bytes in a fresh isolated worker.
pub fn decode(bytes: &[u8]) -> Result<Output> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_INPUT { return Err("RAW 파일 크기가 안전 제한을 초과합니다.".into()); }
    let exe = std::env::var_os("CLIPVIEW_LIBRAW_WORKER").map(std::path::PathBuf::from)
        .map_or_else(std::env::current_exe, Ok).map_err(error)?;
    let output = isolated::run(&exe, bytes, MAX_RGB + 64, std::time::Duration::from_secs(60))?;
    parse(&output)
}

#[cfg(windows)]
mod isolated {
    use std::{ffi::c_void, fs::File, io::{Read, Write}, os::windows::io::FromRawHandle, path::Path, time::Duration};

    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, HLOCAL, LocalFree, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT},
            Security::{
                Authorization::ConvertStringSidToSidW, DuplicateTokenEx, GetLengthSid, PSID, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES,
                SecurityImpersonation, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
                TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel, TokenPrimary,
            },
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP,
                    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS,
                    JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
                    JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
                },
                Pipes::CreatePipe,
                Threading::{
                    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessAsUserW, DeleteProcThreadAttributeList,
                    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
                    LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
                    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
                },
            },
        },
        core::{PCWSTR, PWSTR, w},
    };

    use crate::{Result, error};

    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) { if !self.0.is_invalid() { unsafe { let _ = CloseHandle(self.0); } } }
    }

    fn low_integrity_token() -> Result<Owned> {
        unsafe {
            let mut current = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut current).map_err(error)?;
            let current = Owned(current);
            let mut token = HANDLE::default();
            DuplicateTokenEx(current.0, TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY,
                None, SecurityImpersonation, TokenPrimary, &mut token).map_err(error)?;
            let token = Owned(token);
            let mut sid = PSID::default();
            ConvertStringSidToSidW(w!("S-1-16-4096"), &mut sid).map_err(error)?;
            let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: sid, Attributes: 0x20 /* SE_GROUP_INTEGRITY */ } };
            let result = SetTokenInformation(token.0, TokenIntegrityLevel, std::ptr::from_ref(&label).cast(),
                (size_of::<TOKEN_MANDATORY_LABEL>() as u32) + GetLengthSid(sid));
            let _ = LocalFree(Some(HLOCAL(sid.0)));
            result.map_err(error)?;
            Ok(token)
        }
    }

    fn job(memory: usize) -> Result<Owned> {
        unsafe {
            let job = Owned(CreateJobObjectW(None, PCWSTR::null()).map_err(error)?);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            limits.ProcessMemoryLimit = memory;
            SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, std::ptr::from_ref(&limits).cast(), size_of_val(&limits) as u32).map_err(error)?;
            let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
                UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS | JOB_OBJECT_UILIMIT_EXITWINDOWS
                    | JOB_OBJECT_UILIMIT_GLOBALATOMS | JOB_OBJECT_UILIMIT_HANDLES | JOB_OBJECT_UILIMIT_READCLIPBOARD
                    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
            };
            SetInformationJobObject(job.0, JobObjectBasicUIRestrictions, std::ptr::from_ref(&ui).cast(), size_of_val(&ui) as u32).map_err(error)?;
            Ok(job)
        }
    }

    /// Inheritable pipe whose `keep` end stays non-inheritable in this process.
    fn pipe(child_reads: bool) -> Result<(Owned, Owned)> {
        unsafe {
            let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: true.into() };
            let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
            CreatePipe(&mut read, &mut write, Some(&attributes), 0).map_err(error)?;
            let (read, write) = (Owned(read), Owned(write));
            let (child, parent) = if child_reads { (read, write) } else { (write, read) };
            SetHandleInformation(parent.0, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)).map_err(error)?;
            Ok((child, parent))
        }
    }

    pub fn run(exe: &Path, input: &[u8], output_limit: u64, timeout: Duration) -> Result<Vec<u8>> {
        let token = low_integrity_token()?;
        let job = job(3 << 30)?;
        let (child_in, parent_in) = pipe(true)?;
        let (child_out, parent_out) = pipe(false)?;
        let mut exe_wide: Vec<u16> = exe.as_os_str().encode_wide_vec();
        exe_wide.push(0);
        let mut command: Vec<u16> = format!("\"{}\" {}", exe.display(), super::WORKER_FLAG).encode_utf16().chain([0]).collect();
        let handles = [child_in.0, child_out.0];
        let mut size = 0;
        let mut process = PROCESS_INFORMATION::default();
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
            let mut buffer = vec![0u8; size];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut size).map_err(error)?;
            // Only the two pipe ends are inherited, whatever else is inheritable in this process.
            let updated = UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast::<c_void>()), size_of_val(&handles), None, None);
            let mut startup = STARTUPINFOEXW::default();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = child_in.0;
            startup.StartupInfo.hStdOutput = child_out.0;
            startup.lpAttributeList = list;
            let created = updated.and_then(|_| CreateProcessAsUserW(Some(token.0), PCWSTR(exe_wide.as_ptr()), Some(PWSTR(command.as_mut_ptr())),
                None, None, true, CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT, None, PCWSTR::null(),
                &startup.StartupInfo, &mut process));
            DeleteProcThreadAttributeList(list);
            created.map_err(|e| format!("RAW 작업 프로세스를 시작할 수 없습니다: {e}"))?;
        }
        let (process_handle, thread) = (Owned(process.hProcess), Owned(process.hThread));
        // The worker is confined before it executes a single instruction.
        unsafe {
            if let Err(e) = AssignProcessToJobObject(job.0, process_handle.0) {
                let _ = TerminateJobObject(job.0, 1);
                windows::Win32::System::Threading::TerminateProcess(process_handle.0, 1).ok();
                return Err(format!("RAW 작업 프로세스 격리에 실패했습니다: {e}"));
            }
            ResumeThread(thread.0);
        }
        drop((child_in, child_out));
        let mut writer = unsafe { File::from_raw_handle(parent_in.0.0) };
        std::mem::forget(parent_in);
        let mut reader = unsafe { File::from_raw_handle(parent_out.0.0) };
        std::mem::forget(parent_out);
        let output = std::thread::scope(|scope| {
            let feeder = scope.spawn(move || { let _ = writer.write_all(input); });
            let collector = scope.spawn(move || {
                let mut output = Vec::new();
                Read::take(&mut reader, output_limit + 1).read_to_end(&mut output).map(|_| output)
            });
            let wait = unsafe { WaitForSingleObject(process_handle.0, timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32) };
            if wait != WAIT_OBJECT_0 {
                unsafe { let _ = TerminateJobObject(job.0, 0xE000_0001); }
            }
            let _ = feeder.join();
            let output = collector.join();
            (wait, output)
        });
        let (wait, output) = output;
        if wait == WAIT_TIMEOUT { return Err("RAW 작업 프로세스가 시간 제한을 초과했습니다.".into()); }
        if wait != WAIT_OBJECT_0 { return Err("RAW 작업 프로세스를 기다리지 못했습니다.".into()); }
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(process_handle.0, &mut code).map_err(error)?; }
        let output = output.map_err(|_| "RAW 작업 프로세스 출력 수집 실패".to_owned())?.map_err(error)?;
        match code as i32 {
            0 if output.len() as u64 <= output_limit => Ok(output),
            0 => Err("RAW 작업 프로세스 출력이 안전 제한을 초과했습니다.".into()),
            super::EXIT_INPUT => Err("RAW 작업 프로세스가 입력을 읽지 못했습니다.".into()),
            super::EXIT_DECODE => Err("LibRaw가 이 RAW 파일을 해석하지 못했습니다.".into()),
            super::EXIT_OUTPUT => Err("LibRaw 결과가 안전 제한을 초과했습니다.".into()),
            other => Err(format!("RAW 작업 프로세스가 비정상 종료했습니다 (0x{:08X}).", other as u32)),
        }
    }

    trait WideVec { fn encode_wide_vec(&self) -> Vec<u16>; }
    impl WideVec for std::ffi::OsStr {
        fn encode_wide_vec(&self) -> Vec<u16> { std::os::windows::ffi::OsStrExt::encode_wide(self).collect() }
    }
}

#[cfg(not(windows))]
mod isolated {
    pub fn run(_: &std::path::Path, _: &[u8], _: u64, _: std::time::Duration) -> crate::Result<Vec<u8>> {
        Err("격리된 RAW 작업 프로세스는 Windows에서만 지원합니다.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worker_output_and_rejects_bad_frames() {
        let mut out = Vec::new();
        emit(&mut out, 2, 2, 1, 6, &[255, 0, 0, 0, 0, 255]).unwrap();
        let Output::Image(image) = parse(&out).unwrap() else { panic!("image") };
        // EXIF 6 rotates 90° clockwise: 2x1 becomes 1x2.
        assert_eq!((image.width(), image.height()), (1, 2));
        out[24] ^= 1;
        assert!(parse(&out).is_err());
        assert!(parse(b"CLR1").is_err());
    }

    /// Spawns the real (debug) app executable as the worker: the sandbox must deny a write into
    /// the user's temp folder, and a RAW file must still decode through the pipes.
    #[test]
    #[ignore = "requires CLIPVIEW_LIBRAW_WORKER (debug app build) and CLIP_RAW_SAMPLE_DIR"]
    fn isolated_worker_is_write_restricted_and_decodes() {
        let exe = std::path::PathBuf::from(std::env::var_os("CLIPVIEW_LIBRAW_WORKER").unwrap());
        let target = std::env::temp_dir().join(format!("clipview-sandbox-probe-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&target);
        let probe = format!("CLIPVIEW-PROBE-WRITE:{}", target.display());
        let output = isolated::run(&exe, probe.as_bytes(), 1024, std::time::Duration::from_secs(20)).unwrap();
        assert_eq!(&output[32..], b"DENIED");
        assert!(!target.exists(), "sandboxed worker created {}", target.display());
        let dir = std::path::PathBuf::from(std::env::var_os("CLIP_RAW_SAMPLE_DIR").unwrap());
        let bytes = std::fs::read(dir.join("Panasonic-G9.RW2")).unwrap();
        match decode(&bytes).unwrap() {
            Output::Jpeg { width, height, .. } => assert!(width.max(height) >= MIN_PREVIEW_EDGE),
            Output::Image(image) => assert!(image.width() > 0),
        }
    }

    /// Runs LibRaw in-process on local samples (the isolation itself is tested through the app).
    #[test]
    #[ignore = "requires CLIP_RAW_SAMPLE_DIR"]
    fn libraw_thumbnail_and_development() {
        let dir = std::path::PathBuf::from(std::env::var_os("CLIP_RAW_SAMPLE_DIR").unwrap());
        for name in ["Canon-5D-Mark-IV.CR2", "Panasonic-G9.RW2", "Nikon-Z6.NEF"] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            let (kind, width, height, _, data) = produce(&bytes).unwrap();
            assert!(kind == 1 && width >= MIN_PREVIEW_EDGE.min(height.max(width)), "{name}");
            let mut developed = RawImage::default();
            assert_eq!(unsafe { clipview_libraw_develop(bytes.as_ptr(), bytes.len(), MAX_RAW_MB, 1, &mut developed) }, 0, "{name}");
            assert_eq!(developed.kind, 2);
            assert_eq!(developed.bytes().len(), developed.width as usize * developed.height as usize * 3);
            assert!(!data.is_empty());
            println!("{name}: thumb {width}x{height}, half-size develop {}x{}", developed.width, developed.height);
        }
    }
}
