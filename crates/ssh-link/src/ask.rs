//! The question channel: how a connection asks the person at the glass
//! something, and blocks until they have answered.
//!
//! # Why std channels and not tokio's
//!
//! The asking end is not always inside the runtime, and where it matters
//! most it is inside it in the worst possible way. `HostPolicy::verify` is
//! called from `check_server_key`, which russh calls from inside
//! `client::connect` -- so the ask happens on the runtime's own thread,
//! partway through a handshake, on a stack that cannot be made async
//! without changing russh's trait. Tokio's blocking channel operations
//! panic by design when called from a runtime thread, and for a good
//! reason: they would deadlock a multi-task executor. Here they would not
//! be wrong so much as forbidden.
//!
//! std's channels have no such rule, and blocking this connection's own
//! thread is the correct semantics anyway: a connection waiting on a trust
//! decision has nothing else to do, every other connection has a thread of
//! its own, and the surface that paints the question is a different thread
//! again. What must never block is the surface, and it never does: it only
//! ever calls [`AskDesk::take`], which does not wait.
//!
//! # The ordering law
//!
//! A question and the notice explaining it travel by two different
//! carriers -- the notice on the channel's wire, the question on this desk
//! -- so their order has to be arranged rather than assumed. It is
//! arranged at both ends. The transport puts its `Notice` on the wire
//! *before* it asks; the surface drains the wire's events *before* it
//! drains the desk. So a question can never overtake the line that
//! explains it, and the user is never asked to answer something they have
//! not been told about yet.

use std::sync::mpsc::{channel, Receiver, Sender};

/// What kind of answer a question wants, which is what tells the glass
/// whether it may show the typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Ordinary text, echoed as it is typed.
    Text,
    /// A password, a passphrase, a no-echo challenge: never echoed.
    Secret,
    /// A trust decision, spelled out in full words. Echoed: what makes it
    /// deliberate is the typing, not the hiding.
    YesNo,
}

/// One question, waiting for its answer.
///
/// Answering consumes it, so a question is answered once or not at all,
/// and dropping it unanswered is a cancellation -- which is what makes a
/// surface that loses the connection's row incapable of leaving the
/// asking thread parked forever.
pub struct Question {
    prompt: String,
    kind: Answer,
    reply: Sender<Option<String>>,
}

impl Question {
    /// The question as the glass should print it.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// What kind of answer it wants.
    pub fn kind(&self) -> Answer {
        self.kind
    }

    /// Hand the answer back to whoever asked.
    pub fn answer(self, text: String) {
        let _ = self.reply.send(Some(text));
    }

    /// Withdraw: the asker gets `None` and decides what that means.
    pub fn cancel(self) {
        let _ = self.reply.send(None);
    }
}

/// Hand-written, and the whole point of writing it is what it leaves out.
/// A `Question` never appears in a log or a panic message carrying an
/// answer, because an answer is a password often enough that the
/// distinction is not worth trusting to care. The prompt and the kind are
/// the transport's own words and safe to print.
impl std::fmt::Debug for Question {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Question")
            .field("prompt", &self.prompt)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// What arrives at the desk: something to say, or something to ask.
#[derive(Debug)]
pub enum Ask {
    /// Text for the glass that wants no answer. The connection's own
    /// `Notice` is the usual carrier for this; `Say` is for the words a
    /// policy speaks, which happen on a stack that has no wire in reach.
    Say(String),
    /// A question, blocking its asker until it is answered or cancelled.
    Question(Question),
}

/// The asking end, held by whatever wants an answer. Cloneable, because
/// the policy, the auth sequence and a blocking task all ask through it.
#[derive(Clone)]
pub struct Asker {
    desk: Sender<Ask>,
}

impl Asker {
    /// Ask, and block until the answer comes back.
    ///
    /// `None` is every way of not getting one: the user cancelled, or the
    /// desk is gone because the surface tore the row down. Both mean the
    /// same thing to a caller -- nobody is going to answer this -- so they
    /// are one value rather than two the caller would handle identically.
    pub fn ask(&self, prompt: impl Into<String>, kind: Answer) -> Option<String> {
        let (reply, answer) = channel();
        let question = Question { prompt: prompt.into(), kind, reply };
        if self.desk.send(Ask::Question(question)).is_err() {
            return None;
        }
        answer.recv().ok().flatten()
    }

    /// Say something to the glass without asking anything.
    pub fn say(&self, text: impl Into<String>) {
        let _ = self.desk.send(Ask::Say(text.into()));
    }

    /// An asker with nobody behind it: every question is cancelled the
    /// instant it is asked, and nothing blocks.
    ///
    /// This is what a headless caller hands in -- a test, a tool, any
    /// connection with no glass in front of it. It makes "there is nobody
    /// to ask" an ordinary value rather than an `Option` every call site
    /// would have to unwrap, and the behaviour it produces is the right
    /// one: a connection that cannot ask refuses rather than hangs.
    pub fn closed() -> Self {
        let (desk, _) = channel();
        // The receiver is dropped here; every send fails from now on.
        Self { desk }
    }
}

/// The answering end, held by the surface. Polled, never waited on.
pub struct AskDesk {
    asks: Receiver<Ask>,
}

impl AskDesk {
    /// The next thing waiting, if one is. Never blocks: this is called on
    /// the event loop's own pump, beside every other channel's drain.
    pub fn take(&mut self) -> Option<Ask> {
        self.asks.try_recv().ok()
    }
}

/// A desk and the asker that reaches it.
pub fn desk() -> (Asker, AskDesk) {
    let (tx, rx) = channel();
    (Asker { desk: tx }, AskDesk { asks: rx })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Wait for something to land on the desk, so the test is not a race
    /// against a thread that has only just started.
    fn wait(desk: &mut AskDesk) -> Ask {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(ask) = desk.take() {
                return ask;
            }
            assert!(std::time::Instant::now() < deadline, "nothing reached the desk");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_answer_typed_at_the_desk_is_the_answer_the_asker_gets() {
        let (asker, mut desk) = desk();
        let asking = std::thread::spawn(move || asker.ask("passphrase: ", Answer::Secret));
        let Ask::Question(question) = wait(&mut desk) else {
            panic!("a question was asked, not a saying");
        };
        assert_eq!(question.prompt(), "passphrase: ");
        assert_eq!(question.kind(), Answer::Secret);
        question.answer("tumblers".into());
        assert_eq!(asking.join().unwrap(), Some("tumblers".into()));
    }

    #[test]
    fn a_saying_wants_no_answer_and_blocks_nobody() {
        let (asker, mut desk) = desk();
        asker.say("could not record the key");
        let Ask::Say(text) = wait(&mut desk) else {
            panic!("a saying, not a question");
        };
        assert_eq!(text, "could not record the key");
        assert!(desk.take().is_none(), "and nothing else is waiting");
    }

    #[test]
    fn every_way_of_not_being_answered_is_none() {
        // Cancelled by hand.
        let (asker, mut at_desk) = desk();
        let asking = std::thread::spawn(move || asker.ask("yes or no: ", Answer::YesNo));
        let Ask::Question(question) = wait(&mut at_desk) else { panic!("a question") };
        question.cancel();
        assert_eq!(asking.join().unwrap(), None);

        // The question dropped where it stood: a surface that lost the row.
        let (asker, mut at_desk) = desk();
        let asking = std::thread::spawn(move || asker.ask("yes or no: ", Answer::YesNo));
        drop(wait(&mut at_desk));
        assert_eq!(asking.join().unwrap(), None);

        // The desk itself gone before the question was even asked.
        let (asker, at_desk) = desk();
        drop(at_desk);
        assert_eq!(asker.ask("anyone there: ", Answer::Text), None);
    }

    #[test]
    fn a_closed_asker_refuses_at_once_rather_than_waiting_for_nobody() {
        let asker = Asker::closed();
        let asked = std::time::Instant::now();
        assert_eq!(asker.ask("passphrase: ", Answer::Secret), None);
        assert!(asked.elapsed() < Duration::from_secs(10), "it waited on nobody");
        // And saying something into the void is not an error either.
        asker.say("nothing is listening");
    }

    #[test]
    fn debug_shows_the_question_and_nothing_that_could_be_an_answer() {
        let (asker, mut desk) = desk();
        let asking = std::thread::spawn(move || asker.ask("password: ", Answer::Secret));
        let ask = wait(&mut desk);
        let shown = format!("{ask:?}");
        assert!(shown.contains("password: "), "{shown}");
        assert!(shown.contains("Secret"), "{shown}");
        assert!(!shown.contains("reply"), "{shown}");
        let Ask::Question(question) = ask else { panic!("a question") };
        question.cancel();
        asking.join().unwrap();
    }
}
