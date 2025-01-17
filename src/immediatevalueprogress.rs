use crate::{BoxedSendError, DirectCacheAccess, Progress};
use crate::{ImmediateValuePromise, ImmediateValueState};
use std::borrow::Cow;
use std::time::Instant;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

/// A status update struct containing the issue-date, progress and a message
/// You can use any struct that can be transferred via tokio mpsc channels.
#[derive(Debug)]
pub struct Status<M> {
    /// Time when this status was created
    pub time: Instant,
    /// Current progress
    pub progress: Progress,
    /// Message
    pub message: M,
}

impl<M> Status<M> {
    /// Create a new status message with `now` as timestamp
    pub fn new(progress: Progress, message: M) -> Self {
        Self {
            progress,
            message,
            time: Instant::now(),
        }
    }
}

/// This [`Status`] typedef allows to use both: `&'static str` and `String` in a message
pub type StringStatus = Status<Cow<'static, str>>;

impl StringStatus {
    /// create a [`StringStatus`] from a `&'static str`
    pub fn from_str(progress: Progress, static_message: &'static str) -> Self {
        StringStatus {
            message: Cow::Borrowed(static_message),
            time: Instant::now(),
            progress,
        }
    }
    /// create a [`StringStatus`] from a `String`
    pub fn from_string(progress: Progress, message: String) -> Self {
        StringStatus {
            message: Cow::Owned(message),
            time: Instant::now(),
            progress,
        }
    }
}

/// # A progress and status enabling wrapper for [`ImmediateValuePromise`]
/// This struct allows to use the [`Progress`] type and any kind of status message
/// You can use this to set a computation progress and optionally attach any kind of status message.
/// Assume your action runs  for an extended period of time and you want to inform the user about the state:
///```rust, no_run
///use std::borrow::Cow;
///use std::time::Duration;
///use lazy_async_promise::{ImmediateValueState, ImmediateValuePromise, Progress, ProgressTrackedImValProm, StringStatus};
///let mut oneshot_progress = ProgressTrackedImValProm::new( |s| { ImmediateValuePromise::new(
///  async move {
///  //send some initial status
///    s.send(StringStatus::new(
///      Progress::from_percent(0.0),
///      "Initializing".into(),
///    )).await.unwrap();
///    // do some long running operation
///    for i in 0..100 {
///      tokio::time::sleep(Duration::from_millis(50)).await;
///      s.send(StringStatus::new(
///        Progress::from_percent(i as f64),
///        Cow::Borrowed("In progress"))).await.unwrap();
///    }
///    Ok(34)
///  })}, 2000);
///  assert!(matches!(
///    oneshot_progress.poll_state(),
///    ImmediateValueState::Updating));
///   //waiting and polling will yield "In progress" now :)
/// ```
///
pub struct ProgressTrackedImValProm<T: Send, M> {
    promise: ImmediateValuePromise<T>,
    status: Vec<Status<M>>,
    receiver: Receiver<Status<M>>,
}

impl<T: Send + 'static, M> ProgressTrackedImValProm<T, M> {
    /// create a new Progress tracked immediate value promise.
    pub fn new(
        creator: impl FnOnce(Sender<Status<M>>) -> ImmediateValuePromise<T>,
        buffer: usize,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(buffer);
        ProgressTrackedImValProm {
            receiver,
            status: Vec::new(),
            promise: creator(sender),
        }
    }

    /// Slice of all recorded [`Status`] changes
    pub fn status_history(&self) -> &[Status<M>] {
        &self.status
    }

    /// Get the last [`Status`] if there is any
    pub fn last_status(&self) -> Option<&Status<M>> {
        self.status.last()
    }

    /// Is our future already finished?
    pub fn finished(&self) -> bool {
        self.promise.get_value().is_some()
    }

    /// Poll the state and process the messages
    pub fn poll_state(&mut self) -> &ImmediateValueState<T> {
        while let Ok(msg) = self.receiver.try_recv() {
            self.status.push(msg);
        }
        self.promise.poll_state()
    }

    /// Get the current progress
    pub fn get_progress(&self) -> Progress {
        self.status
            .last()
            .map(|p| p.progress)
            .unwrap_or(Progress::default())
    }
}

impl<T: Send + 'static, M> DirectCacheAccess<T, BoxedSendError> for ProgressTrackedImValProm<T, M> {
    fn get_value_mut(&mut self) -> Option<&mut T> {
        self.promise.get_value_mut()
    }
    fn get_value(&self) -> Option<&T> {
        self.promise.get_value()
    }
    fn get_result(&self) -> Option<Result<&T, &BoxedSendError>> {
        self.promise.get_result()
    }
    fn take_value(&mut self) -> Option<T> {
        self.promise.take_value()
    }
    fn take_result(&mut self) -> Option<Result<T, BoxedSendError>> {
        self.promise.take_result()
    }
}
#[cfg(test)]
mod test {
    use super::*;
    use crate::ImmediateValuePromise;
    use std::time::Duration;
    #[tokio::test]
    async fn basic_usage_cycle() {
        let mut oneshot_progress = ProgressTrackedImValProm::new(
            |s| {
                ImmediateValuePromise::new(async move {
                    s.send(StringStatus::from_str(
                        Progress::from_percent(0.0),
                        "Initializing",
                    ))
                    .await
                    .unwrap();
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    s.send(StringStatus::new(
                        Progress::from_percent(50.0),
                        "processing".into(),
                    ))
                    .await
                    .unwrap();
                    tokio::time::sleep(Duration::from_millis(25)).await;

                    s.send(StringStatus::from_string(
                        Progress::from_percent(100.0),
                        format!("Done"),
                    ))
                    .await
                    .unwrap();
                    Ok(34)
                })
            },
            2000,
        );
        assert!(matches!(
            oneshot_progress.poll_state(),
            ImmediateValueState::Updating
        ));
        assert!(!oneshot_progress.finished());

        assert_eq!(*oneshot_progress.get_progress(), 0.0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = oneshot_progress.poll_state();
        assert_eq!(*oneshot_progress.get_progress(), 1.0);
        let result = oneshot_progress.poll_state();

        if let ImmediateValueState::Success(val) = result {
            assert_eq!(*val, 34);
        } else {
            unreachable!();
        }
        // check finished
        assert!(oneshot_progress.finished());
        let history = oneshot_progress.status_history();
        assert_eq!(history.len(), 3);

        // check direct cache access trait
        let val = oneshot_progress.get_value().unwrap();
        assert_eq!(*val, 34);
        let val = oneshot_progress.get_value_mut().unwrap();
        *val = 33;
        assert_eq!(*oneshot_progress.get_value().unwrap(), 33);
        let val = oneshot_progress.take_value().unwrap();
        assert_eq!(val, 33);
        assert!(oneshot_progress.get_value().is_none());
    }
}
