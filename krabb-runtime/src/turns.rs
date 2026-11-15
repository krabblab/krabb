use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard};

pub(crate) const STREAK: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Interactive,
    Background,
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    waiting: VecDeque<(u64, Class)>,
    streak: u32,
}

impl State {
    fn head(&self) -> Option<u64> {
        let first = |class| self.waiting.iter().find(|(_, waiting)| *waiting == class);

        match (first(Class::Interactive), first(Class::Background)) {
            (Some(_), Some((id, _))) if self.streak >= STREAK => Some(*id),
            (Some((id, _)), _) => Some(*id),
            (None, background) => background.map(|(id, _)| *id),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Turns {
    state: Mutex<State>,
    changed: Notify,
}

struct Ticket<'a> {
    turns: &'a Turns,
    id: u64,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.turns.state.lock() {
            state.waiting.retain(|(id, _)| *id != self.id);
        }

        self.turns.changed.notify_waiters();
    }
}

impl Turns {
    /// Serves interactive requests first while bounding background starvation.
    pub async fn acquire<T>(&self, class: Class, lock: Arc<AsyncMutex<T>>) -> OwnedMutexGuard<T> {
        let id = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let id = state.next;
            state.next = state.next.wrapping_add(1);
            state.waiting.push_back((id, class));
            id
        };

        let ticket = Ticket { turns: self, id };

        self.changed.notify_waiters();

        let head = || self.state.lock().unwrap_or_else(|error| error.into_inner()).head();

        let guard = 'admission: loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if head() != Some(id) {
                notified.await;
                continue;
            }

            let locking = Arc::clone(&lock).lock_owned();
            tokio::pin!(locking);

            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();

                if head() != Some(id) {
                    continue 'admission;
                }

                tokio::select! {
                    biased;
                    guard = &mut locking => break 'admission guard,

                    _ = &mut notified => {}
                }
            }
        };

        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let deferred = state
                .waiting
                .iter()
                .any(|(other, waiting)| *other != id && *waiting == Class::Background);

            state.streak = match class {
                Class::Interactive if deferred => state.streak.saturating_add(1),
                _ => 0,
            };
        }

        drop(ticket);
        guard
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.state.lock().unwrap().waiting.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn queue(
        turns: &Arc<Turns>,
        lock: &Arc<AsyncMutex<()>>,
        order: &Arc<Mutex<Vec<String>>>,
        name: &str,
        class: Class,
    ) -> tokio::task::JoinHandle<()> {
        let expected = turns.waiting() + 1;
        let (turns, lock, order, name) =
            (Arc::clone(turns), Arc::clone(lock), Arc::clone(order), name.to_owned());
        let observed = Arc::clone(&turns);

        let task = tokio::spawn(async move {
            let _guard = turns.acquire(class, lock).await;
            order.lock().unwrap().push(name);
            tokio::time::sleep(Duration::from_millis(5)).await;
        });

        while observed.waiting() < expected {
            tokio::task::yield_now().await;
        }

        task
    }

    #[tokio::test]
    async fn interactive_turns_lead_while_background_waits_a_bounded_streak() {
        let turns = Arc::new(Turns::default());
        let lock = Arc::new(AsyncMutex::new(()));
        let order = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::clone(&lock).lock_owned().await;
        let mut tasks = Vec::new();

        tasks.push(queue(&turns, &lock, &order, "background-1", Class::Background).await);

        for index in 1..=4 {
            let name = format!("interactive-{index}");
            tasks.push(queue(&turns, &lock, &order, &name, Class::Interactive).await);
        }

        tasks.push(queue(&turns, &lock, &order, "background-2", Class::Background).await);
        drop(held);

        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(
            *order.lock().unwrap(),
            [
                "interactive-1",
                "interactive-2",
                "interactive-3",
                "background-1",
                "interactive-4",
                "background-2"
            ]
        );

        assert_eq!(turns.waiting(), 0);
    }

    #[tokio::test]
    async fn a_waiting_head_keeps_its_place_on_the_lock_when_others_arrive() {
        let turns = Arc::new(Turns::default());
        let lock = Arc::new(AsyncMutex::new(()));
        let order = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::clone(&lock).lock_owned().await;

        let head = queue(&turns, &lock, &order, "interactive", Class::Interactive).await;

        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        let direct = {
            let (lock, order) = (Arc::clone(&lock), Arc::clone(&order));

            tokio::spawn(async move {
                let _guard = lock.lock_owned().await;
                order.lock().unwrap().push("direct".to_owned());
            })
        };

        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        let background = queue(&turns, &lock, &order, "background", Class::Background).await;

        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        drop(held);

        for task in [head, direct, background] {
            task.await.unwrap();
        }

        assert_eq!(*order.lock().unwrap(), ["interactive", "direct", "background"]);
    }

    #[tokio::test]
    async fn an_expired_waiter_leaves_the_queue_without_blocking_others() {
        let turns = Arc::new(Turns::default());
        let lock = Arc::new(AsyncMutex::new(()));
        let held = Arc::clone(&lock).lock_owned().await;

        let expired = tokio::time::timeout(
            Duration::from_millis(20),
            turns.acquire(Class::Interactive, Arc::clone(&lock)),
        )
        .await;

        assert!(expired.is_err());
        assert_eq!(turns.waiting(), 0);

        let order = Arc::new(Mutex::new(Vec::new()));
        let background = queue(&turns, &lock, &order, "background", Class::Background).await;

        drop(held);
        tokio::time::timeout(Duration::from_secs(2), background).await.unwrap().unwrap();

        assert_eq!(*order.lock().unwrap(), ["background"]);
    }
}
