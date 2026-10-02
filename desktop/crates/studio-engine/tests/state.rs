use studio_engine::*;
use studio_project::{ProjectId, SourceRevision};
fn rev(c: char) -> SourceRevision {
    SourceRevision::try_from(c.to_string().repeat(64)).unwrap()
}
fn state(id: &str, session: u8) -> ProjectState {
    let mut s = ProjectState::opening(
        ProjectId::try_from(id.to_owned()).unwrap(),
        OpenSession([session; 16]),
        rev('a'),
        rev('b'),
    );
    s.finish_open(Ok(())).unwrap();
    s
}
fn running(s: &mut ProjectState, kind: JobKind) -> OperationTag {
    let tag = s.queue(kind).unwrap();
    s.start(&tag).unwrap();
    tag
}
#[test]
fn lifecycle_and_checkpoint_are_distinct_from_build_and_candidate() {
    let mut s = state("one", 1);
    let tag = running(&mut s, JobKind::Build);
    assert!(s.queue(JobKind::Build).is_err());
    s.complete(&tag, rev('a'), JobResult::Built(rev('a')))
        .unwrap();
    assert_eq!(s.built(), Some(&rev('a')));
    assert_eq!(s.accepted(), &rev('b'));
    s.propose(&tag, rev('a'), rev('c')).unwrap();
    assert_eq!(s.candidate().unwrap().revision(), &rev('c'));
    assert_eq!(s.accepted(), &rev('b'));
    s.reconcile_source(rev('d')).unwrap();
    assert!(s.candidate().is_none());
    assert!(s.built().is_none());
    assert_eq!(s.accepted(), &rev('b'));
    let checkpoint = running(&mut s, JobKind::Checkpoint);
    s.complete(&checkpoint, rev('d'), JobResult::Checkpointed(rev('d')))
        .unwrap();
    assert_eq!(s.accepted(), &rev('d'));
    s.close();
    assert_eq!(s.open_state(), &OpenState::Closed);
    assert!(s.queue(JobKind::Build).is_err());
}
#[test]
fn every_completion_tag_is_checked_without_installing() {
    for field in 0..5 {
        let mut s = state("one", 1);
        let tag = running(&mut s, JobKind::Checkpoint);
        let mut wrong = tag.clone();
        match field {
            0 => wrong.project = ProjectId::try_from("two".to_owned()).unwrap(),
            1 => wrong.session = OpenSession([2; 16]),
            2 => wrong.base_source = rev('c'),
            3 => wrong.operation = OperationId(999),
            _ => wrong.generation += 1,
        }
        assert_eq!(
            s.complete(&wrong, rev('a'), JobResult::Checkpointed(rev('a'))),
            Err(StateError::StaleResult)
        );
        assert_eq!(s.accepted(), &rev('b'));
        s.complete(&tag, rev('a'), JobResult::Checkpointed(rev('a')))
            .unwrap();
    }
}
#[test]
fn external_edit_and_edit_back_reject_late_results() {
    let mut s = state("one", 1);
    let tag = running(&mut s, JobKind::Build);
    assert_eq!(
        s.complete(&tag, rev('c'), JobResult::Built(rev('a'))),
        Err(StateError::StaleResult)
    );
    assert_eq!(s.source(), &rev('c'));
    assert!(matches!(s.job(), JobState::Interrupted(_)));
    s.reconcile_source(rev('a')).unwrap();
    assert_eq!(
        s.complete(&tag, rev('a'), JobResult::Built(rev('a'))),
        Err(StateError::StaleResult)
    );
    assert!(s.built().is_none());
}
#[test]
fn cancellation_failure_and_interruption_transitions() {
    let mut s = state("one", 1);
    let t = s.queue(JobKind::Build).unwrap();
    assert_eq!(
        s.complete(&t, rev('a'), JobResult::Built(rev('a'))),
        Err(StateError::StaleResult)
    );
    s.start(&t).unwrap();
    assert!(
        s.complete(&t, rev('a'), JobResult::Checkpointed(rev('a')))
            .is_err()
    );
    s.request_cancel().unwrap();
    assert!(matches!(s.job(), JobState::CancelRequested(_)));
    assert!(
        s.complete(&t, rev('a'), JobResult::Built(rev('a')))
            .is_err()
    );
    s.interrupt().unwrap();
    let t = running(&mut s, JobKind::Build);
    s.complete(&t, rev('a'), JobResult::Failed("compiler error".into()))
        .unwrap();
    assert!(matches!(s.job(), JobState::Failed(_, _)));
    let t = running(&mut s, JobKind::Build);
    s.close();
    assert!(
        s.complete(&t, rev('a'), JobResult::Built(rev('a')))
            .is_err()
    );
    let mut opening = ProjectState::opening(
        ProjectId::try_from("one".to_owned()).unwrap(),
        OpenSession([3; 16]),
        rev('a'),
        rev('b'),
    );
    assert!(opening.queue(JobKind::Build).is_err());
    opening.finish_open(Err("missing file".into())).unwrap();
    assert!(matches!(opening.open_state(), OpenState::Error(_)));
    assert!(opening.finish_open(Ok(())).is_err());
}
#[test]
fn simultaneous_projects_and_reopen_are_isolated() {
    let mut a = state("one", 1);
    let mut b = state("two", 2);
    let tag = running(&mut a, JobKind::Build);
    running(&mut b, JobKind::Build);
    assert!(
        b.complete(&tag, rev('a'), JobResult::Built(rev('a')))
            .is_err()
    );
    a.reconcile_source(rev('c')).unwrap();
    assert_eq!(b.source(), &rev('a'));
    a.close();
    let mut reopened = state("one", 3);
    running(&mut reopened, JobKind::Build);
    assert!(
        reopened
            .complete(&tag, rev('a'), JobResult::Built(rev('a')))
            .is_err()
    );
}

#[test]
fn new_open_sessions_never_reuse_identity() {
    assert_ne!(OpenSession::new(), OpenSession::new());
}
