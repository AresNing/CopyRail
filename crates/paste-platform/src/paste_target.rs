//! The production paste state machine, with OS effects behind a small adapter.
//! Identity is an application *instance*, never just a reusable process id.
use crate::ClipboardError;

#[derive(Clone)]
pub(crate) struct Invocation<T> {
    generation: u64,
    pub(crate) target: Option<T>,
}

pub(crate) struct TargetSession<T> {
    generation: u64,
    target: Option<T>,
}

impl<T> Default for TargetSession<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            target: None,
        }
    }
}

impl<T: Clone> TargetSession<T> {
    pub(crate) fn replace(&mut self, target: Option<T>) {
        self.generation = self.generation.wrapping_add(1);
        self.target = target;
    }

    pub(crate) fn snapshot(&self) -> Invocation<T> {
        Invocation {
            generation: self.generation,
            target: self.target.clone(),
        }
    }

    pub(crate) fn is_current(&self, invocation: &Invocation<T>) -> bool {
        self.generation == invocation.generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteOutcome {
    /// The event pair was submitted. This does not acknowledge text insertion.
    Requested,
    NoTarget,
    SessionChanged,
    TargetUnavailable,
    FocusChanged,
    ClipboardChanged,
    PermissionDenied,
    ActivationRefused,
    ActivationTimedOut,
    PostFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteProgress {
    Pending,
    Complete(PasteOutcome),
}

pub(crate) trait PasteEnvironment<T> {
    fn trusted(&self) -> bool;
    fn clipboard_change_count(&self) -> i64;
    fn is_live(&self, target: &T) -> bool;
    fn frontmost(&self) -> Option<T>;
    fn is_self(&self, application: &T) -> bool;
    fn activate(&mut self, target: &T) -> bool;
    fn post(&mut self, target: &T) -> Result<(), ClipboardError>;
}

pub(crate) struct PasteAttempt<T> {
    invocation: Invocation<T>,
    clipboard_change_count: i64,
    activation_requested: bool,
    completed: Option<PasteOutcome>,
}

impl<T: Clone + Eq> PasteAttempt<T> {
    pub(crate) fn clipboard_change_count(&self) -> i64 {
        self.clipboard_change_count
    }

    pub(crate) fn new(invocation: Invocation<T>, clipboard_change_count: i64) -> Self {
        Self {
            invocation,
            clipboard_change_count,
            activation_requested: false,
            completed: None,
        }
    }

    fn finish(&mut self, outcome: PasteOutcome) -> PasteProgress {
        self.completed = Some(outcome);
        PasteProgress::Complete(outcome)
    }

    /// Called once per main-run-loop turn; never sleeps or spins in AppKit.
    pub(crate) fn advance(
        &mut self,
        session: &TargetSession<T>,
        environment: &mut impl PasteEnvironment<T>,
        timed_out: bool,
    ) -> Result<PasteProgress, ClipboardError> {
        if let Some(completed) = self.completed {
            return Ok(PasteProgress::Complete(completed));
        }
        if !session.is_current(&self.invocation) {
            return Ok(self.finish(PasteOutcome::SessionChanged));
        }
        let Some(target) = self.invocation.target.clone() else {
            return Ok(self.finish(PasteOutcome::NoTarget));
        };
        if !environment.trusted() {
            return Ok(self.finish(PasteOutcome::PermissionDenied));
        }
        if environment.clipboard_change_count() != self.clipboard_change_count {
            return Ok(self.finish(PasteOutcome::ClipboardChanged));
        }
        if !environment.is_live(&target) || environment.is_self(&target) {
            return Ok(self.finish(PasteOutcome::TargetUnavailable));
        }
        if timed_out {
            return Ok(self.finish(PasteOutcome::ActivationTimedOut));
        }
        let frontmost = environment.frontmost();
        if frontmost
            .as_ref()
            .is_some_and(|front| front != &target && !environment.is_self(front))
        {
            return Ok(self.finish(PasteOutcome::FocusChanged));
        }
        if frontmost.as_ref() == Some(&target) {
            // Recheck immediately before the irreversible effect. A changed
            // pasteboard must not send newly copied, unrelated user content.
            if environment.clipboard_change_count() != self.clipboard_change_count {
                return Ok(self.finish(PasteOutcome::ClipboardChanged));
            }
            if !environment.is_live(&target) {
                return Ok(self.finish(PasteOutcome::TargetUnavailable));
            }
            self.completed = Some(PasteOutcome::PostFailed);
            environment.post(&target)?;
            return Ok(self.finish(PasteOutcome::Requested));
        }
        if frontmost.is_none() {
            return Ok(PasteProgress::Pending);
        }
        if !self.activation_requested {
            if !environment.activate(&target) {
                return Ok(self.finish(PasteOutcome::ActivationRefused));
            }
            self.activation_requested = true;
        }
        Ok(PasteProgress::Pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same pid, distinct instance: mirrors NSRunningApplication equality.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Application {
        pid: i32,
        instance: u64,
    }
    fn app(instance: u64) -> Application {
        Application { pid: 42, instance }
    }
    struct Environment {
        live: Option<Application>,
        front: Option<Application>,
        count: i64,
        permission: bool,
        activation: bool,
        activated: usize,
        posted: usize,
        post_fails: bool,
    }
    impl PasteEnvironment<Application> for Environment {
        fn trusted(&self) -> bool {
            self.permission
        }
        fn clipboard_change_count(&self) -> i64 {
            self.count
        }
        fn is_live(&self, target: &Application) -> bool {
            self.live.as_ref() == Some(target)
        }
        fn frontmost(&self) -> Option<Application> {
            self.front.clone()
        }
        fn is_self(&self, application: &Application) -> bool {
            application.instance == 0
        }
        fn activate(&mut self, _: &Application) -> bool {
            self.activated += 1;
            self.activation
        }
        fn post(&mut self, _: &Application) -> Result<(), ClipboardError> {
            if self.post_fails {
                return Err(ClipboardError::EventCreationFailed);
            }
            self.posted += 1;
            Ok(())
        }
    }
    fn fixture() -> (
        TargetSession<Application>,
        PasteAttempt<Application>,
        Environment,
    ) {
        let mut session = TargetSession::default();
        session.replace(Some(app(1)));
        let attempt = PasteAttempt::new(session.snapshot(), 100);
        (
            session,
            attempt,
            Environment {
                live: Some(app(1)),
                front: Some(app(0)),
                count: 100,
                permission: true,
                activation: true,
                activated: 0,
                posted: 0,
                post_fails: false,
            },
        )
    }

    #[test]
    fn menu_invocation_replaces_old_target_and_unknown_invocation_clears_it() {
        let (mut session, mut old, mut env) = fixture();
        session.replace(Some(app(2)));
        assert_eq!(
            old.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Complete(PasteOutcome::SessionChanged)
        );
        assert_eq!(session.snapshot().target, Some(app(2)));
        session.replace(None);
        let mut unknown = PasteAttempt::new(session.snapshot(), 100);
        assert_eq!(
            unknown.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Complete(PasteOutcome::NoTarget)
        );
        assert_eq!((env.activated, env.posted), (0, 0));
    }

    #[test]
    fn waits_for_actual_foreground_and_only_posts_once() {
        let (session, mut attempt, mut env) = fixture();
        for _ in 0..4 {
            assert_eq!(
                attempt.advance(&session, &mut env, false).expect("step"),
                PasteProgress::Pending
            );
        }
        assert_eq!((env.activated, env.posted), (1, 0));
        env.front = Some(app(1));
        for _ in 0..2 {
            assert_eq!(
                attempt.advance(&session, &mut env, false).expect("step"),
                PasteProgress::Complete(PasteOutcome::Requested)
            );
        }
        assert_eq!(env.posted, 1);
    }

    #[test]
    fn exited_target_or_reused_pid_never_receives_events() {
        for (waiting, live) in [
            (false, None),
            (false, Some(app(2))),
            (true, None),
            (true, Some(app(2))),
        ] {
            let (session, mut attempt, mut env) = fixture();
            if waiting {
                let _ = attempt.advance(&session, &mut env, false).expect("step");
            }
            env.live = live;
            assert_eq!(
                attempt.advance(&session, &mut env, false).expect("step"),
                PasteProgress::Complete(PasteOutcome::TargetUnavailable)
            );
            assert_eq!((env.activated, env.posted), (usize::from(waiting), 0));
        }
    }

    #[test]
    fn unknown_foreground_waits_without_forcing_activation() {
        let (session, mut attempt, mut env) = fixture();
        env.front = None;
        assert_eq!(
            attempt.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Pending
        );
        assert_eq!((env.activated, env.posted), (0, 0));
        assert_eq!(
            attempt.advance(&session, &mut env, true).expect("step"),
            PasteProgress::Complete(PasteOutcome::ActivationTimedOut)
        );
    }

    #[test]
    fn already_frontmost_target_needs_no_activation_request() {
        let (session, mut attempt, mut env) = fixture();
        env.front = Some(app(1));
        assert_eq!(
            attempt.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Complete(PasteOutcome::Requested)
        );
        assert_eq!((env.activated, env.posted), (0, 1));
    }

    #[test]
    fn expired_request_cannot_post_even_if_target_eventually_becomes_frontmost() {
        let (session, mut attempt, mut env) = fixture();
        assert_eq!(
            attempt.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Pending
        );
        env.front = Some(app(1));
        assert_eq!(
            attempt.advance(&session, &mut env, true).expect("step"),
            PasteProgress::Complete(PasteOutcome::ActivationTimedOut)
        );
        assert_eq!(env.posted, 0);
    }

    #[test]
    fn our_own_application_is_not_a_paste_destination() {
        let (mut session, _, mut env) = fixture();
        session.replace(Some(app(0)));
        env.live = Some(app(0));
        let mut attempt = PasteAttempt::new(session.snapshot(), 100);
        assert_eq!(
            attempt.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Complete(PasteOutcome::TargetUnavailable)
        );
        assert_eq!((env.activated, env.posted), (0, 0));
    }

    #[test]
    fn focus_switch_is_not_overridden_before_or_during_activation() {
        for waiting in [false, true] {
            let (session, mut attempt, mut env) = fixture();
            if waiting {
                let _ = attempt.advance(&session, &mut env, false).expect("step");
            }
            env.front = Some(app(2));
            assert_eq!(
                attempt.advance(&session, &mut env, false).expect("step"),
                PasteProgress::Complete(PasteOutcome::FocusChanged)
            );
            assert_eq!(env.posted, 0);
            assert_eq!(env.activated, usize::from(waiting));
        }
    }

    #[test]
    fn permission_and_clipboard_changes_abort_before_and_after_activation() {
        for waiting in [false, true] {
            for outcome in [
                PasteOutcome::PermissionDenied,
                PasteOutcome::ClipboardChanged,
            ] {
                let (session, mut attempt, mut env) = fixture();
                if waiting {
                    let _ = attempt.advance(&session, &mut env, false).expect("step");
                }
                if outcome == PasteOutcome::PermissionDenied {
                    env.permission = false;
                } else {
                    env.count += 1;
                }
                env.front = Some(app(1));
                assert_eq!(
                    attempt.advance(&session, &mut env, false).expect("step"),
                    PasteProgress::Complete(outcome)
                );
                assert_eq!(env.posted, 0);
            }
        }
    }

    #[test]
    fn refusal_timeout_and_unknown_foreground_do_not_fake_success() {
        for (front, activation, timeout, expected) in [
            (Some(app(0)), false, false, PasteOutcome::ActivationRefused),
            (Some(app(0)), true, true, PasteOutcome::ActivationTimedOut),
            (None, true, true, PasteOutcome::ActivationTimedOut),
        ] {
            let (session, mut attempt, mut env) = fixture();
            env.front = front;
            env.activation = activation;
            assert_eq!(
                attempt.advance(&session, &mut env, timeout).expect("step"),
                PasteProgress::Complete(expected)
            );
            assert_eq!(env.posted, 0);
        }
    }

    #[test]
    fn hide_or_reopen_invalidates_even_the_same_target() {
        for replacement in [None, Some(app(1))] {
            let (mut session, mut attempt, mut env) = fixture();
            let _ = attempt.advance(&session, &mut env, false).expect("step");
            session.replace(replacement);
            env.front = Some(app(1));
            assert_eq!(
                attempt.advance(&session, &mut env, false).expect("step"),
                PasteProgress::Complete(PasteOutcome::SessionChanged)
            );
            assert_eq!(env.posted, 0);
        }
    }

    #[test]
    fn failed_event_creation_is_terminal_without_retries() {
        let (session, mut attempt, mut env) = fixture();
        env.front = Some(app(1));
        env.post_fails = true;
        assert!(attempt.advance(&session, &mut env, false).is_err());
        env.post_fails = false;
        assert_eq!(
            attempt.advance(&session, &mut env, false).expect("step"),
            PasteProgress::Complete(PasteOutcome::PostFailed)
        );
        assert_eq!(env.posted, 0);
    }
}
