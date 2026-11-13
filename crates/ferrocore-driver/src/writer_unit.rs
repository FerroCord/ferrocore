use super::*;
use std::sync::{Arc, Condvar, Mutex, mpsc as std_mpsc};
use std::thread;

struct BlockingLog {
    bytes: Arc<Mutex<Vec<u8>>>,
    started: Option<std_mpsc::Sender<()>>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl Write for BlockingLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(started) = self.started.take() {
            started.send(()).unwrap();
            let (lock, wake) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released { released = wake.wait(released).unwrap(); }
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl LogFile for BlockingLog {
    fn sync_data(&self) -> std::io::Result<()> { Ok(()) }
}

fn append_command(reply: Reply<Uuid>, consumed: oneshot::Receiver<()>) -> Command {
    Command::Append {
        parent_id: None,
        agent_depth: 0,
        loop_type: "rlm".into(),
        kind: "session_start".into(),
        data: serde_json::json!({}),
        reply,
        consumed,
    }
}

fn lost_acknowledgement(after_delivery: bool) {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (started_tx, started_rx) = std_mpsc::channel();
    let file = BlockingLog { bytes: bytes.clone(), started: Some(started_tx), release: release.clone() };
    let (sender, receiver) = mpsc::channel(8);
    let worker = thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().build().unwrap()
            .block_on(writer_task(file, receiver, String::new()));
    });
    let (reply, response) = oneshot::channel();
    let (consumed, receipt) = oneshot::channel();
    assert!(sender.try_send(append_command(reply, receipt)).is_ok());
    started_rx.recv().unwrap();

    let (later_reply, later_response) = oneshot::channel();
    let (later_consumed, later_receipt) = oneshot::channel();
    assert!(sender.try_send(append_command(later_reply, later_receipt)).is_ok());
    let (sync_reply, sync_response) = oneshot::channel();
    assert!(sender.try_send(Command::Sync(sync_reply)).is_ok());
    let (close_reply, close_response) = oneshot::channel();
    assert!(sender.try_send(Command::Close(close_reply)).is_ok());

    let mut response = Some(response);
    if !after_delivery { drop(response.take()); }
    {
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_one();
    }
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    if after_delivery {
        // Receive the delivered reply while withholding the consumption receipt.
        assert!(runtime.block_on(response.take().unwrap()).unwrap().is_ok());
    }
    drop(consumed);
    runtime.block_on(async {
        assert_eq!(later_response.await.unwrap(), Err(LogError::AcknowledgementLost));
        let _ = later_consumed.send(());
        assert_eq!(sync_response.await.unwrap(), Err(LogError::AcknowledgementLost));
        assert_eq!(close_response.await.unwrap(), Err(LogError::AcknowledgementLost));
    });
    worker.join().unwrap();
    let written = bytes.lock().unwrap();
    let lines: Vec<_> = written.split(|&byte| byte == b'\n').filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    let record: LogRecord = serde_json::from_slice(lines[0]).unwrap();
    assert_eq!(record.payload.kind, "session_start");
}

#[test]
fn cancellation_during_write_fails_stop() { lost_acknowledgement(false); }

#[test]
fn delivered_but_unconsumed_acknowledgement_fails_stop() { lost_acknowledgement(true); }
