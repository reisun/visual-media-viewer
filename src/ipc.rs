use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

const IPC_PORT: u16 = 52384;

pub fn try_send_to_existing(path: &PathBuf) -> bool {
    let addr = format!("127.0.0.1:{}", IPC_PORT);
    if let Ok(mut stream) = TcpStream::connect(&addr) {
        #[cfg(target_os = "windows")]
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX).ok();
        }
        let msg = path.to_string_lossy();
        let _ = writeln!(stream, "{}", msg);
        return true;
    }
    false
}

pub fn start_listener() -> mpsc::Receiver<PathBuf> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", IPC_PORT);
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(_) => return,
        };
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stream);
                for line in reader.lines().flatten() {
                    let path = PathBuf::from(line.trim());
                    if path.exists() {
                        let _ = tx.send(path);
                    }
                }
            });
        }
    });
    rx
}
