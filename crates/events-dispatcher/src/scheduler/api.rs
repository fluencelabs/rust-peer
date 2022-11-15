use fluence_libp2p::types::Outlet;
use std::time::Duration;
use thiserror::Error;

pub struct TimerConfig {
    pub period: Duration,
}

pub enum Command {
    Add { id: String, config: TimerConfig },
    Remove { id: String },
}

#[derive(Error, Debug)]
pub enum SchedulerError {
    #[error("can't send a message to the scheduler")]
    CommandSendError,
}

pub struct SchedulerApi {
    send_command: Outlet<Command>,
}

impl SchedulerApi {
    pub fn new(send_command: Outlet<Command>) -> Self {
        Self { send_command }
    }

    fn send(&self, command: Command) -> Result<(), SchedulerError> {
        self.send_command
            .unbounded_send(command)
            .map_err(|_| SchedulerError::CommandSendError)
    }

    pub fn add(&self, id: String, config: TimerConfig) -> Result<(), SchedulerError> {
        self.send(Command::Add { id, config })
    }

    pub fn remove(&self, id: String) -> Result<(), SchedulerError> {
        self.send(Command::Remove { id })
    }
}
