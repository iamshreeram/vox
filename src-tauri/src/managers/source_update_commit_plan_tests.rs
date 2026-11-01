use super::*;

const PENDINGS: [PendingInput; 4] = [
    PendingInput::Absent,
    PendingInput::Invalid,
    PendingInput::Built,
    PendingInput::Committing,
];

const PROBES: [JobProbe; 4] = [
    JobProbe::Absent,
    JobProbe::Registered {
        pid: None,
        last_exit: Some(0),
    },
    JobProbe::Registered {
        pid: Some(42),
        last_exit: None,
    },
    JobProbe::Unknown,
];

const STATUSES: [StatusInput; 3] = [
    StatusInput::None,
    StatusInput::NonTerminal,
    StatusInput::Terminal,
];

fn plan(pending: PendingInput, probe: JobProbe, install_status: StatusInput) -> RecoveryAction {
    plan_recovery(&RecoveryInput {
        pending,
        probe,
        install_status,
    })
}

const ADOPT: RecoveryAction = RecoveryAction::AdoptInstall {
    hold_admission: true,
};

/// Explicit table for the `committing` record: (probe, status, expected).
#[test]
fn committing_truth_table() {
    let rows = [
        // No install status: only a conclusively absent label may be revoked.
        (
            JobProbe::Absent,
            StatusInput::None,
            RecoveryAction::RevokeInstall,
        ),
        (PROBES[1], StatusInput::None, ADOPT),
        (PROBES[2], StatusInput::None, ADOPT),
        (JobProbe::Unknown, StatusInput::None, ADOPT),
        // Non-terminal status: always adopt, whatever the probe says.
        (JobProbe::Absent, StatusInput::NonTerminal, ADOPT),
        (PROBES[1], StatusInput::NonTerminal, ADOPT),
        (PROBES[2], StatusInput::NonTerminal, ADOPT),
        (JobProbe::Unknown, StatusInput::NonTerminal, ADOPT),
        // Terminal status: release only when no process can still be live.
        (
            JobProbe::Absent,
            StatusInput::Terminal,
            RecoveryAction::FinishInstall,
        ),
        (
            PROBES[1],
            StatusInput::Terminal,
            RecoveryAction::FinishInstall,
        ),
        (PROBES[2], StatusInput::Terminal, ADOPT),
        (JobProbe::Unknown, StatusInput::Terminal, ADOPT),
    ];
    for (probe, status, expected) in rows {
        assert_eq!(
            plan(PendingInput::Committing, probe, status),
            expected,
            "committing with {probe:?} and {status:?}"
        );
    }
}

#[test]
fn non_committing_states_ignore_probe_and_status() {
    for probe in PROBES {
        for status in STATUSES {
            assert_eq!(
                plan(PendingInput::Absent, probe, status),
                RecoveryAction::NothingToDo
            );
            assert_eq!(
                plan(PendingInput::Invalid, probe, status),
                RecoveryAction::DiscardInvalid
            );
            assert_eq!(
                plan(PendingInput::Built, probe, status),
                RecoveryAction::RestBuilt
            );
        }
    }
}

#[test]
fn full_grid_is_total_and_unknown_never_releases_or_deletes_a_commit() {
    for pending in PENDINGS {
        for probe in PROBES {
            for status in STATUSES {
                let action = plan(pending, probe, status);
                if probe.is_unknown() && pending == PendingInput::Committing {
                    assert!(
                        !matches!(
                            action,
                            RecoveryAction::FinishInstall
                                | RecoveryAction::RevokeInstall
                                | RecoveryAction::DiscardInvalid
                        ),
                        "Unknown must never release or delete: {pending:?} {status:?} -> {action:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn adopt_always_holds_admission() {
    for probe in PROBES {
        for status in STATUSES {
            if let RecoveryAction::AdoptInstall { hold_admission } =
                plan(PendingInput::Committing, probe, status)
            {
                assert!(hold_admission);
            }
        }
    }
}
