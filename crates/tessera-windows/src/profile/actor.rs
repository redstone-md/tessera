// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, mpsc};

use tessera_system::profile::{
    ProfileCommand, ProfileError, ProfileErrorKind, ProfileHost, ProfileOpenCompletion,
    ProfileReadCompletion, ProfileSnapshot,
};

const QUEUE_CAPACITY: usize = 16;

/// Drivers, apartments and native allocations never leave their dedicated owner.
pub(crate) trait Driver: 'static {
    fn read(&mut self) -> Result<ProfileSnapshot, ProfileError>;
    fn execute(&mut self, command: ProfileCommand) -> Result<(), ProfileError>;
}

enum Request {
    Read(ProfileReadCompletion),
    Open(ProfileCommand, ProfileOpenCompletion),
}

struct Host<F> {
    create: Arc<F>,
    sender: Mutex<Option<mpsc::SyncSender<Request>>>,
}

pub(crate) fn host<D, F>(create: F) -> Arc<dyn ProfileHost>
where
    D: Driver,
    F: Fn() -> Result<D, ProfileError> + Send + Sync + 'static,
{
    Arc::new(Host {
        create: Arc::new(create),
        sender: Mutex::new(None),
    })
}

impl<F> Host<F> {
    fn admit<D>(&self, request: Request) -> Result<(), ProfileError>
    where
        D: Driver,
        F: Fn() -> Result<D, ProfileError> + Send + Sync + 'static,
    {
        let mut slot = self.sender.lock().map_err(|_| unavailable())?;
        if slot.is_none() {
            let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
            let create = Arc::clone(&self.create);
            std::thread::Builder::new()
                .name("tessera-profile".into())
                .spawn(move || run(create.as_ref(), receiver))
                .map_err(|_| unavailable())?;
            *slot = Some(sender);
        }
        let sender = slot.as_ref().ok_or_else(unavailable)?;
        sender.try_send(request).map_err(|failure| match failure {
            mpsc::TrySendError::Full(_) => ProfileError::new(
                ProfileErrorKind::Busy,
                "The profile request queue is full.",
                None,
            ),
            mpsc::TrySendError::Disconnected(_) => unavailable(),
        })
    }
}

impl<D, F> ProfileHost for Host<F>
where
    D: Driver,
    F: Fn() -> Result<D, ProfileError> + Send + Sync + 'static,
{
    fn read(&self, completion: ProfileReadCompletion) -> Result<(), ProfileError> {
        self.admit::<D>(Request::Read(completion))
    }

    fn execute(
        &self,
        command: ProfileCommand,
        completion: ProfileOpenCompletion,
    ) -> Result<(), ProfileError> {
        self.admit::<D>(Request::Open(command, completion))
    }
}

fn run<D: Driver>(
    create: &impl Fn() -> Result<D, ProfileError>,
    receiver: mpsc::Receiver<Request>,
) {
    // Waiting for accepted work is not polling and does not initialize COM.
    let Ok(first) = receiver.recv() else { return };
    let mut driver = catch_unwind(AssertUnwindSafe(create)).unwrap_or_else(|_| Err(unavailable()));
    process(&mut driver, first);
    for request in receiver {
        process(&mut driver, request);
    }
    // Last sender drop drains accepted commands; native cleanup stays here.
}

fn process<D: Driver>(driver: &mut Result<D, ProfileError>, request: Request) {
    match request {
        Request::Read(completion) => {
            let result = guarded(driver, Driver::read);
            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
        }
        Request::Open(command, completion) => {
            let result = guarded(driver, |driver| driver.execute(command));
            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
        }
    }
}

fn guarded<D, T>(
    driver: &mut Result<D, ProfileError>,
    operation: impl FnOnce(&mut D) -> Result<T, ProfileError>,
) -> Result<T, ProfileError> {
    let result = match driver {
        Ok(driver) => catch_unwind(AssertUnwindSafe(|| operation(driver))),
        Err(error) => return Err(error.clone()),
    };
    match result {
        Ok(result) => result,
        Err(_) => {
            // A panicked backend is never reused; all queued completions settle.
            *driver = Err(unavailable());
            Err(unavailable())
        }
    }
}

fn unavailable() -> ProfileError {
    ProfileError::new(
        ProfileErrorKind::Other,
        "The profile worker is unavailable.",
        None,
    )
}
