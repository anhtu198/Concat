// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! One chat turn: Claude Code started with the message on stdin, its
//! stream read line by line on a thread of its own, and a way to stop it.
//! Whatever happens, the turn ends with exactly one `Done`.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::stream::{ChatEvent, parse_line};

type Sink = Arc<dyn Fn(ChatEvent) + Send + Sync>;

/// A running chat turn.
pub struct Runner {
    shared: Arc<Shared>,
}

struct Shared {
    child: Mutex<Option<Child>>,
    /// Holds the child and everything it starts; closing it kills them, so
    /// a turn cannot outlive its runner (the window closing included).
    #[cfg(windows)]
    job: job::Job,
    stopped: AtomicBool,
    finished: AtomicBool,
    sink: Sink,
}

impl Shared {
    /// Passes an event on unless the turn is over; the first `Done` ends it.
    fn emit(&self, event: ChatEvent) {
        if self.finished.load(Ordering::SeqCst) {
            return;
        }
        if matches!(event, ChatEvent::Done { .. }) && self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        (self.sink)(event);
    }
}

impl Runner {
    /// Starts `program` with `args`, writes `stdin_text` to it and closes
    /// its stdin, and hands every event to `on_event` from a reader thread.
    pub fn start(
        program: PathBuf,
        args: Vec<OsString>,
        stdin_text: String,
        cwd: Option<PathBuf>,
        on_event: impl Fn(ChatEvent) + Send + Sync + 'static,
    ) -> std::io::Result<Runner> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up beside the editor.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        if let Some(mut stdin) = child.stdin.take() {
            // A child that exits before reading is reported by its exit.
            let _ = stdin.write_all(stdin_text.as_bytes());
        }
        #[cfg(windows)]
        let job = {
            use std::os::windows::io::AsRawHandle;
            let job = job::Job::new();
            job.assign(child.as_raw_handle());
            job
        };
        let shared = Arc::new(Shared {
            #[cfg(windows)]
            job,
            child: Mutex::new(Some(child)),
            stopped: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            sink: Arc::new(on_event),
        });

        let errors = Arc::new(Mutex::new(String::new()));
        let err_sink = Arc::clone(&errors);
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            *err_sink.lock().unwrap() = text;
        });

        let reader = Arc::clone(&shared);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                for event in parse_line(&line) {
                    reader.emit(event);
                }
            }
            let status = reader
                .child
                .lock()
                .unwrap()
                .as_mut()
                .and_then(|child| child.wait().ok());
            if reader.stopped.load(Ordering::SeqCst) || reader.finished.load(Ordering::SeqCst) {
                reader.emit(ChatEvent::Done { session_id: None });
                return;
            }
            // Give the stderr reader a moment to finish its copy.
            std::thread::sleep(std::time::Duration::from_millis(50));
            let said = errors.lock().unwrap().trim().to_owned();
            let message = if said.is_empty() {
                match status.and_then(|status| status.code()) {
                    Some(code) => format!("exit code {code}"),
                    None => "the process ended without an answer".to_owned(),
                }
            } else {
                let tail: Vec<char> = said.chars().rev().take(500).collect();
                tail.into_iter().rev().collect()
            };
            reader.emit(ChatEvent::Error(message));
            reader.emit(ChatEvent::Done { session_id: None });
        });
        Ok(Runner { shared })
    }

    /// Ends the turn now: the process (and, on Windows, every process it
    /// started) is killed and the turn's `Done` is sent at once.
    pub fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        // The column hears the turn ended before anything is killed.
        self.shared.emit(ChatEvent::Done { session_id: None });
        #[cfg(windows)]
        self.shared.job.terminate();
        if let Ok(mut child) = self.shared.child.try_lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
    }

    /// Whether the turn has not ended yet.
    pub fn is_running(&self) -> bool {
        !self.shared.finished.load(Ordering::SeqCst)
    }
}

#[cfg(windows)]
mod job {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// A Windows job object that kills its processes when closed. A null
    /// handle (creation failed) makes every call a no-op; the child is
    /// still killed directly.
    pub struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Job {
            // SAFETY: plain Win32 calls with valid (null or owned) arguments.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if !handle.is_null() {
                    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                    SetInformationJobObject(
                        handle,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as *const c_void,
                        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                }
                Job(handle)
            }
        }

        pub fn assign(&self, process: *mut c_void) {
            if !self.0.is_null() {
                // SAFETY: `process` is the live child's handle.
                unsafe { AssignProcessToJobObject(self.0, process as HANDLE) };
            }
        }

        pub fn terminate(&self) {
            if !self.0.is_null() {
                // SAFETY: an owned job handle.
                unsafe { TerminateJobObject(self.0, 1) };
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: closed once, here; KILL_ON_JOB_CLOSE ends the tree.
                unsafe { CloseHandle(self.0) };
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn collect(rx: &mpsc::Receiver<ChatEvent>) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(10)) {
            let done = matches!(event, ChatEvent::Done { .. });
            out.push(event);
            if done {
                break;
            }
        }
        out
    }

    fn start_cmd(args: &[&str]) -> (Runner, mpsc::Receiver<ChatEvent>) {
        let (tx, rx) = mpsc::channel();
        let runner = Runner::start(
            "cmd".into(),
            args.iter().map(OsString::from).collect(),
            "hello".into(),
            None,
            move |event| {
                let _ = tx.send(event);
            },
        )
        .unwrap();
        (runner, rx)
    }

    #[test]
    fn streams_text_then_done() {
        let dir = std::env::temp_dir().join(format!("concat-chat-runner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("stream.jsonl");
        std::fs::write(
            &fixture,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"xong\"}]}}\n\
             {\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"s9\"}\n",
        )
        .unwrap();
        let (_runner, rx) = start_cmd(&["/C", "type", fixture.to_str().unwrap()]);
        assert_eq!(
            collect(&rx),
            vec![
                ChatEvent::Text("xong".into()),
                ChatEvent::Done {
                    session_id: Some("s9".into())
                }
            ]
        );
    }

    #[test]
    fn exit_without_result_reports_error() {
        let (_runner, rx) = start_cmd(&["/C", "exit 3"]);
        let events = collect(&rx);
        assert!(
            matches!(&events[0], ChatEvent::Error(text) if text.contains('3')),
            "{events:?}"
        );
        assert_eq!(events.last(), Some(&ChatEvent::Done { session_id: None }));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn stop_kills_and_ends() {
        let (runner, rx) = start_cmd(&["/C", "ping -n 30 127.0.0.1 >NUL"]);
        std::thread::sleep(Duration::from_millis(200));
        assert!(runner.is_running());
        runner.stop();
        let events = collect(&rx);
        assert_eq!(events, vec![ChatEvent::Done { session_id: None }]);
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "only one Done"
        );
        assert!(!runner.is_running());
    }
}
