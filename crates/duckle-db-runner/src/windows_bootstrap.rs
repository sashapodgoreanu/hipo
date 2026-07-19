//! Windows-only inherited-pipe bootstrap transport.
//!
//! The child inherits exactly two handles through
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`: one read endpoint for the parent
//! bootstrap message and one write endpoint for the non-secret control reply.
//! Credentials are never placed in command-line arguments, the environment,
//! a readiness file, or standard I/O.

use crate::bootstrap::{write_bootstrap, BootstrapError, BootstrapMessage};
use std::ffi::{c_void, OsStr, OsString};
use std::fs::File;
use std::mem::{size_of, size_of_val, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use thiserror::Error;
use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTUPINFOEXW,
};

#[derive(Debug, Error)]
pub enum WindowsBootstrapError {
    #[error("Windows sidecar process API failed")]
    Os(#[source] std::io::Error),
    #[error("sidecar executable path is not absolute")]
    RelativeProgram,
    #[error("sidecar command-line argument contains an interior NUL")]
    InteriorNul,
    #[error("sidecar bootstrap payload failed")]
    Bootstrap(#[from] BootstrapError),
}

/// Parent-side ownership of a child started with the two permitted inherited
/// pipe handles. The bootstrap writer is consumed exactly once and closed
/// immediately after writing to make trailing-data detection deterministic.
pub struct SpawnedSidecar {
    bootstrap_writer: Option<File>,
    control_reader: File,
    process: OwnedHandle,
    #[allow(dead_code)] // owning it keeps KILL_ON_JOB_CLOSE active
    job: OwnedHandle,
    process_id: u32,
}

impl SpawnedSidecar {
    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    pub fn send_bootstrap(
        &mut self,
        message: &BootstrapMessage,
    ) -> Result<(), WindowsBootstrapError> {
        let mut writer = self.bootstrap_writer.take().ok_or_else(|| {
            WindowsBootstrapError::Os(std::io::Error::other("bootstrap already sent"))
        })?;
        let result = write_bootstrap(&mut writer, message);
        drop(writer);
        result.map_err(WindowsBootstrapError::Bootstrap)
    }

    /// The control stream may contain only non-secret readiness metadata. It
    /// remains provider-private and must never be forwarded as a DTO.
    #[allow(dead_code)] // consumed by LocalProcessProvider readiness handling
    pub(crate) fn control_reader(&mut self) -> &mut File {
        &mut self.control_reader
    }

    #[allow(dead_code)] // consumed by process containment and termination
    pub(crate) fn process_handle(&self) -> RawHandle {
        self.process.as_raw_handle()
    }

    /// Terminates the complete Job Object, including descendants. Closing the
    /// job is a second safety net through `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    pub fn terminate_tree(&mut self) -> Result<(), WindowsBootstrapError> {
        let terminated = unsafe {
            // SAFETY: `job` remains a live Job Object owned by this guard.
            TerminateJobObject(self.job.as_raw_handle() as HANDLE, 1)
        };
        if terminated == 0 {
            return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

impl crate::process_cleanup::ProcessTreeHandle for SpawnedSidecar {
    fn terminate_tree(&mut self) -> std::io::Result<()> {
        SpawnedSidecar::terminate_tree(self)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

/// Starts an absolute sidecar executable. The only additional arguments are
/// the numerical values of inherited handles; they are capabilities to no
/// resource by themselves and do not carry the bootstrap credential.
pub fn spawn_sidecar(
    program: &Path,
    additional_arguments: &[OsString],
) -> Result<SpawnedSidecar, WindowsBootstrapError> {
    if !program.is_absolute() {
        return Err(WindowsBootstrapError::RelativeProgram);
    }
    let mut pipes = BootstrapPipes::new()?;
    let mut arguments = Vec::with_capacity(additional_arguments.len() + 5);
    arguments.push(program.as_os_str().to_os_string());
    arguments.extend_from_slice(additional_arguments);
    arguments.push(OsString::from("--duckle-bootstrap-read-handle"));
    arguments.push(OsString::from(
        (pipes
            .child_bootstrap_reader
            .as_ref()
            .expect("child bootstrap handle")
            .as_raw_handle() as usize as u64)
            .to_string(),
    ));
    arguments.push(OsString::from("--duckle-control-write-handle"));
    arguments.push(OsString::from(
        (pipes
            .child_control_writer
            .as_ref()
            .expect("child control handle")
            .as_raw_handle() as usize as u64)
            .to_string(),
    ));

    let mut command_line = build_command_line(&arguments)?;
    let program_wide = wide_null(program.as_os_str())?;
    let inherited = [
        pipes
            .child_bootstrap_reader
            .as_ref()
            .expect("child bootstrap handle")
            .as_raw_handle() as HANDLE,
        pipes
            .child_control_writer
            .as_ref()
            .expect("child control handle")
            .as_raw_handle() as HANDLE,
    ];
    let attributes = AttributeList::new(&inherited)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.as_ptr();
    let mut process_info: PROCESS_INFORMATION = unsafe {
        // SAFETY: a zeroed PROCESS_INFORMATION is the documented input state
        // for CreateProcessW, whose fields are initialized on success.
        zeroed()
    };

    let created = unsafe {
        // SAFETY: the application and command line are NUL-terminated UTF-16,
        // the attribute list points to the two live child handles, and all
        // other pointers are either valid initialized structures or null as
        // documented by CreateProcessW.
        CreateProcessW(
            program_wide.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT,
            std::ptr::null(),
            std::ptr::null(),
            &startup.StartupInfo,
            &mut process_info,
        )
    };
    if created == 0 {
        return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
    }

    let process = unsafe {
        // SAFETY: CreateProcessW returned success and transferred ownership of
        // the process handle to this parent-side guard.
        OwnedHandle::from_raw_handle(process_info.hProcess as RawHandle)
    };
    let thread = unsafe {
        // SAFETY: CreateProcessW returned success and transferred ownership of
        // the primary thread handle, which is closed immediately below.
        OwnedHandle::from_raw_handle(process_info.hThread as RawHandle)
    };
    drop(thread);
    let job = match create_kill_on_close_job(process.as_raw_handle() as HANDLE) {
        Ok(job) => job,
        Err(error) => {
            unsafe {
                // SAFETY: the child process handle is valid and owned by this
                // parent. No credential has been written to its pipe yet.
                TerminateProcess(process.as_raw_handle() as HANDLE, 1);
            }
            return Err(error);
        }
    };
    pipes.close_child_ends();
    Ok(SpawnedSidecar {
        bootstrap_writer: Some(pipes.parent_bootstrap_writer),
        control_reader: pipes.parent_control_reader,
        process,
        job,
        process_id: process_info.dwProcessId,
    })
}

fn create_kill_on_close_job(process: HANDLE) -> Result<OwnedHandle, WindowsBootstrapError> {
    let raw_job = unsafe {
        // SAFETY: null attributes/name request an unnamed job local to this
        // process; the returned handle is checked before ownership transfer.
        CreateJobObjectW(std::ptr::null(), std::ptr::null())
    };
    if raw_job.is_null() {
        return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
    }
    let job = unsafe {
        // SAFETY: CreateJobObjectW returned a live, uniquely owned handle.
        OwnedHandle::from_raw_handle(raw_job as RawHandle)
    };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        // SAFETY: `limits` is initialized and valid for the exact structure
        // size required by JobObjectExtendedLimitInformation.
        SetInformationJobObject(
            job.as_raw_handle() as HANDLE,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
    }
    let assigned = unsafe {
        // SAFETY: both handles are live and owned by this parent; the child
        // has not received a bootstrap credential before containment is set.
        AssignProcessToJobObject(job.as_raw_handle() as HANDLE, process)
    };
    if assigned == 0 {
        return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
    }
    Ok(job)
}

/// Takes ownership of the child endpoints after parsing the two numeric
/// handles supplied by `spawn_sidecar`. This is crate-private because external
/// runtimes must never receive either pipe or the bootstrap credential.
#[allow(dead_code)] // consumed by the trusted sidecar entrypoint
pub(crate) unsafe fn take_child_pipes(
    bootstrap_read_handle: usize,
    control_write_handle: usize,
) -> (File, File) {
    // SAFETY: the caller is the trusted sidecar entrypoint and passes exactly
    // the two handles inherited from `spawn_sidecar`; each is consumed once.
    let bootstrap = unsafe { File::from_raw_handle(bootstrap_read_handle as RawHandle) };
    // SAFETY: see the preceding safety contract for the inherited control
    // handle; it is distinct from the bootstrap endpoint.
    let control = unsafe { File::from_raw_handle(control_write_handle as RawHandle) };
    (bootstrap, control)
}

struct BootstrapPipes {
    parent_bootstrap_writer: File,
    parent_control_reader: File,
    child_bootstrap_reader: Option<OwnedHandle>,
    child_control_writer: Option<OwnedHandle>,
}

impl BootstrapPipes {
    fn new() -> Result<Self, WindowsBootstrapError> {
        let (bootstrap_reader, bootstrap_writer) = create_inheritable_pipe()?;
        let (control_reader, control_writer) = create_inheritable_pipe()?;
        clear_inherit_flag(bootstrap_writer.as_raw_handle() as HANDLE)?;
        clear_inherit_flag(control_reader.as_raw_handle() as HANDLE)?;
        let parent_bootstrap_writer = File::from(bootstrap_writer);
        let parent_control_reader = File::from(control_reader);
        Ok(Self {
            parent_bootstrap_writer,
            parent_control_reader,
            child_bootstrap_reader: Some(bootstrap_reader),
            child_control_writer: Some(control_writer),
        })
    }

    fn close_child_ends(&mut self) {
        drop(self.child_bootstrap_reader.take());
        drop(self.child_control_writer.take());
    }
}

fn create_inheritable_pipe() -> Result<(OwnedHandle, OwnedHandle), WindowsBootstrapError> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut reader = std::ptr::null_mut();
    let mut writer = std::ptr::null_mut();
    let created = unsafe {
        // SAFETY: CreatePipe initializes both HANDLE outputs on success. The
        // SECURITY_ATTRIBUTES pointer is valid for this call and requests
        // inheritance only for the two handles subsequently allowlisted.
        CreatePipe(&mut reader, &mut writer, &attributes, 0)
    };
    if created == 0 {
        return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
    }
    let reader = unsafe {
        // SAFETY: CreatePipe succeeded and ownership of the new handle moves
        // into the RAII wrapper exactly once.
        OwnedHandle::from_raw_handle(reader as RawHandle)
    };
    let writer = unsafe {
        // SAFETY: CreatePipe succeeded and ownership of the new handle moves
        // into the RAII wrapper exactly once.
        OwnedHandle::from_raw_handle(writer as RawHandle)
    };
    Ok((reader, writer))
}

fn clear_inherit_flag(handle: HANDLE) -> Result<(), WindowsBootstrapError> {
    let changed = unsafe {
        // SAFETY: `handle` is owned by this process and remains live for the
        // duration of SetHandleInformation.
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0)
    };
    if changed == 0 {
        return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
    }
    Ok(())
}

struct AttributeList {
    storage: Vec<usize>,
    pointer: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttributeList {
    fn new(handles: &[HANDLE; 2]) -> Result<Self, WindowsBootstrapError> {
        let mut bytes = 0_usize;
        unsafe {
            // SAFETY: the null first call requests the allocation size only.
            // Windows documents a failing return for this sizing call.
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let pointer = storage.as_mut_ptr().cast::<c_void>();
        let initialized = unsafe {
            // SAFETY: storage is pointer-aligned, has the size requested by
            // Windows, and remains owned by AttributeList until Drop.
            InitializeProcThreadAttributeList(pointer, 1, 0, &mut bytes)
        };
        if initialized == 0 {
            return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
        }
        let updated = unsafe {
            // SAFETY: the list was initialized above and `handles` points to
            // exactly the two live child endpoints permitted for inheritance.
            UpdateProcThreadAttribute(
                pointer,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast::<c_void>(),
                size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if updated == 0 {
            unsafe {
                // SAFETY: the list was initialized and is not used after this
                // cleanup path.
                DeleteProcThreadAttributeList(pointer);
            }
            return Err(WindowsBootstrapError::Os(std::io::Error::last_os_error()));
        }
        Ok(Self { storage, pointer })
    }

    fn as_ptr(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        let _ = &self.storage; // keeps the backing allocation live for Windows
        self.pointer
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: a successful constructor initialized this list, and the
            // backing storage remains live until after this drop returns.
            DeleteProcThreadAttributeList(self.pointer);
        }
    }
}

fn wide_null(value: &OsStr) -> Result<Vec<u16>, WindowsBootstrapError> {
    let mut output = value.encode_wide().collect::<Vec<_>>();
    if output.contains(&0) {
        return Err(WindowsBootstrapError::InteriorNul);
    }
    output.push(0);
    Ok(output)
}

fn build_command_line(arguments: &[OsString]) -> Result<Vec<u16>, WindowsBootstrapError> {
    let mut command = String::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index > 0 {
            command.push(' ');
        }
        command.push_str(&quote_windows_argument(&argument.to_string_lossy()));
    }
    wide_null(OsStr::new(&command))
}

fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument.to_string();
    }
    let mut quoted = String::from("\"");
    let mut slashes = 0_usize;
    for character in argument.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(slashes.saturating_mul(2).saturating_add(1)));
                quoted.push('"');
                slashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(slashes));
                quoted.push(character);
                slashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(slashes.saturating_mul(2)));
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn bootstrap_pipes_are_directional() {
        let mut pipes = BootstrapPipes::new().unwrap();
        pipes
            .parent_bootstrap_writer
            .write_all(b"parent-to-child")
            .unwrap();
        pipes.parent_bootstrap_writer.flush().unwrap();
        let mut received = [0_u8; 15];
        let mut child_reader = File::from(pipes.child_bootstrap_reader.take().unwrap());
        child_reader.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"parent-to-child");

        let mut child_writer = File::from(pipes.child_control_writer.take().unwrap());
        child_writer.write_all(b"child-to-parent").unwrap();
        child_writer.flush().unwrap();
        let mut control = [0_u8; 15];
        pipes
            .parent_control_reader
            .read_exact(&mut control)
            .unwrap();
        assert_eq!(&control, b"child-to-parent");
    }

    #[test]
    fn quotes_windows_arguments_without_losing_backslashes() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument("two words"), "\"two words\"");
        assert_eq!(quote_windows_argument("a\\\"b"), "\"a\\\\\\\"b\"");
    }
}
