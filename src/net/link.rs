//! A TCP connection with optional added latency: messages go out through a writer thread that
//! holds each one for the lag, and come in through a reader thread.

use crate::proto::{Msg, encode, read_msg};
use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct Link {
    out: Option<mpsc::Sender<(Instant, Vec<u8>)>>,
    rx: mpsc::Receiver<io::Result<Msg>>,
    writer: Option<JoinHandle<()>>,
    lag: Duration,
    /// Bytes handed to the network so far.
    pub sent: u64,
    /// Bytes received so far.
    pub received: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Link {
    pub fn new(stream: TcpStream, lag: Duration) -> io::Result<Link> {
        stream.set_nodelay(true)?;
        let mut reader = stream.try_clone()?;
        let received = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let count = received.clone();
        let (in_tx, rx) = mpsc::channel();
        thread::spawn(move || {
            loop {
                let m = read_msg(&mut reader);
                if let Ok(m) = &m {
                    count.fetch_add(encode(m).len() as u64, std::sync::atomic::Ordering::Relaxed);
                }
                let end = m.is_err();
                if in_tx.send(m).is_err() || end {
                    break;
                }
            }
        });
        let (out, out_rx) = mpsc::channel::<(Instant, Vec<u8>)>();
        let mut w = stream;
        let writer = thread::spawn(move || {
            for (due, bytes) in out_rx {
                let now = Instant::now();
                if due > now {
                    thread::sleep(due - now);
                }
                if w.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        Ok(Link { out: Some(out), rx, writer: Some(writer), lag, sent: 0, received })
    }

    pub fn send(&mut self, msg: &Msg) {
        let bytes = encode(msg);
        self.sent += bytes.len() as u64;
        if let Some(out) = &self.out {
            let _ = out.send((Instant::now() + self.lag, bytes));
        }
    }

    /// The next message, waiting for it.
    pub fn recv(&self) -> io::Result<Msg> {
        self.rx.recv().map_err(|_| io::Error::new(io::ErrorKind::ConnectionAborted, "connection closed"))?
    }

    /// The next message if one is waiting.
    pub fn try_recv(&self) -> Option<io::Result<Msg>> {
        self.rx.try_recv().ok()
    }

    /// Sends what is queued, then closes the sending side.
    pub fn close(&mut self) {
        self.out = None;
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }

    pub fn received(&self) -> u64 {
        self.received.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}
