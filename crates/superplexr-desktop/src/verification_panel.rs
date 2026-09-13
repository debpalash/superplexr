//! Desktop review controls; execution/collection stay in the shared module and
//! blocking work stays on the background executor.
use super::*;
use superplexr_verification::{LaunchTarget, Setup};

#[derive(Clone, Copy)]
enum Choice {
    Plan,
    Evidence,
    Runner,
}

impl SuperplexrDesktop {
    fn verification_setup(&self, mission: MissionId, run: RunId) -> Setup {
        self.verification_runs
            .get(&run)
            .or_else(|| self.verification_setups.get(&mission))
            .cloned()
            .unwrap_or_else(|| Setup {
                runner_path: std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.join("superplexr")))
                    .unwrap_or_default(),
                ..Setup::default()
            })
    }

    fn choose_verification_path(
        &mut self,
        mission: MissionId,
        run: RunId,
        choice: Choice,
        cx: &mut Context<Self>,
    ) {
        if self.shared_mode || self.review_action_busy {
            return;
        }
        self.review_action_busy = true;
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: !matches!(choice, Choice::Evidence),
            directories: matches!(choice, Choice::Evidence),
            multiple: false,
            prompt: Some(
                match choice {
                    Choice::Plan => "Choose trusted check plan",
                    Choice::Evidence => "Choose private evidence folder",
                    Choice::Runner => "Choose SuperPlexr CLI executable",
                }
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let result = picker.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.review_action_busy = false;
                if desktop.shared_mode {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            let mut setup = desktop.verification_setup(mission, run);
                            match choice {
                                Choice::Plan => setup.plan_path = path,
                                Choice::Evidence => setup.evidence_root = path,
                                Choice::Runner => setup.runner_path = path,
                            }
                            desktop.verification_setups.insert(mission, setup.clone());
                            if desktop.verification_runs.contains_key(&run) {
                                desktop.verification_runs.insert(run, setup);
                            }
                            desktop.verification_preview = None;
                            desktop.review_action_status = Some((
                                true,
                                "File choice saved. Review the plan before running checks.".into(),
                            ));
                            desktop.persist_workspaces();
                        }
                    }
                    Ok(Ok(None)) => {}
                    error => {
                        desktop.review_action_status =
                            Some((false, format!("File picker unavailable: {error:?}")))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn review_verification_plan(&mut self, mission: MissionId, run: RunId, cx: &mut Context<Self>) {
        if self.shared_mode || self.review_action_busy {
            return;
        }
        let setup = self.verification_setup(mission, run);
        let for_prepare = setup.clone();
        self.review_action_busy = true;
        self.verification_preview = None;
        self.review_action_status =
            Some((true, "Checking plan and local file permissions…".into()));
        let request = cx
            .background_executor()
            .spawn(async move { superplexr_verification::prepare(&for_prepare) });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.review_action_busy = false;
                if desktop.verification_setup(mission, run) != setup {
                    return;
                }
                match result {
                    Ok(plan) => {
                        desktop.verification_preview = Some((mission, run, plan));
                        desktop.review_action_status = Some((
                            true,
                            "Review all six commands below. Nothing has executed.".into(),
                        ));
                    }
                    Err(error) => desktop.review_action_status = Some((false, error.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run_reviewed_checks(
        &mut self,
        mission_id: MissionId,
        run: RunId,
        target: LaunchTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared_mode || self.review_action_busy {
            return;
        }
        let Some((mission, selected, plan)) = self.verification_preview.clone() else {
            return;
        };
        if mission != mission_id || selected != run {
            return;
        }
        let (target, attempted_verifier) = match target {
            LaunchTarget::Subject(subject) => {
                let verifier = RunId::new();
                (LaunchTarget::NewVerifier { subject, verifier }, verifier)
            }
            LaunchTarget::NewVerifier { verifier, .. } | LaunchTarget::Verifier(verifier) => {
                (target, verifier)
            }
        };
        let warning = format!(
            "These commands run with your OS-user permissions, including file and credential access. This is not a sandbox.\n\nPlan: {}\nSHA-256: {}\nVerifier attempt: {}\n\nThis starts a verifier only. It never accepts or merges work.",
            plan.setup.plan_path.display(),
            plan.sha256,
            attempted_verifier
        );
        let confirmation = window.prompt(
            PromptLevel::Warning,
            "Run reviewed checks?",
            Some(&warning),
            &["Run checks", "Cancel"],
            cx,
        );
        let control = self.control.clone();
        self.review_action_busy = true;
        cx.spawn(async move |this, cx| {
            if confirmation.await.ok() != Some(0) {
                let _ = this.update(cx, |desktop, cx| {
                    desktop.review_action_busy = false;
                    cx.notify();
                });
                return;
            }
            let setup = plan.setup.clone();
            let request = cx.background_executor().spawn(async move {
                superplexr_verification::launch(&control, mission_id, target, &plan)
            });
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.review_action_busy = false;
                desktop.verification_preview = None;
                match result {
                    Ok(outcome) => {
                        desktop.verification_runs.insert(outcome.run_id, setup);
                        desktop.apply_review_mission(outcome.mission);
                        if desktop.workspace().mission_id == Some(mission_id) {
                            desktop.selected_graph_run = Some(outcome.run_id);
                            desktop.refresh_mission_history(cx);
                        }
                        desktop.review_action_status = Some(match outcome.launch_error {
                            Some(error) => (
                                false,
                                format!(
                                    "Verifier {} retained; launch failed: {error}",
                                    short_id(outcome.run_id)
                                ),
                            ),
                            None => (
                                true,
                                "Checks running. Collect evidence after this verifier finishes."
                                    .into(),
                            ),
                        });
                        desktop.persist_workspaces();
                    }
                    Err(error) => desktop.review_action_status = Some((false, format!(
                        "Verifier attempt {attempted_verifier}: {error}. Inspect this Run ID before retrying; creation or launch may be unconfirmed."
                    ))),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn apply_review_mission(&mut self, mission: Mission) {
        for tab in self.workspaces.tabs_mut() {
            let view = tab.content_mut();
            if view.mission_id == Some(mission.id)
                && view
                    .mission
                    .as_ref()
                    .is_none_or(|current| current.version <= mission.version)
            {
                view.mission = Some(mission.clone());
            }
        }
    }

    fn collect_verification(&mut self, mission: MissionId, run: RunId, cx: &mut Context<Self>) {
        if self.shared_mode || self.review_action_busy {
            return;
        }
        let setup = self.verification_setup(mission, run);
        let control = self.control.clone();
        self.review_action_busy = true;
        self.review_action_status =
            Some((true, "Validating and collecting retained evidence…".into()));
        let request = cx.background_executor().spawn(async move {
            let receipt =
                superplexr_verification::collect(&control, mission, run, &setup.evidence_root)
                    .map_err(|error| error.to_string())?;
            let state = control
                .get_mission(mission)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((receipt, state))
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.review_action_busy = false;
                match result {
                    Ok((receipt, state)) => {
                        desktop.apply_review_mission(state);
                        if desktop.workspace().mission_id == Some(mission) {
                            desktop.selected_graph_run = Some(receipt.subject_run_id);
                            desktop.refresh_mission_history(cx);
                        }
                        desktop.review_action_status = Some((
                            receipt.verdict == superplexr_core::EvaluationVerdict::Passed,
                            format!(
                                "Evidence collected: {:?}. Owner acceptance remains separate.",
                                receipt.verdict
                            ),
                        ));
                    }
                    Err(error) => desktop.review_action_status = Some((false, error)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_verification_panel(
        &self,
        mission: &Mission,
        run_id: RunId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let run = &mission.runs[&run_id];
        let mission_id = mission.id;
        let input = mission
            .verified_delivery
            .delivery_run_inputs
            .get(&run_id)
            .filter(|input| input.purpose == DeliveryRunPurpose::Verification);
        let source = run.disposition == superplexr_core::RunDisposition::AwaitingReview
            && mission.verified_delivery.candidates.contains_key(&run_id);
        if !source && input.is_none() {
            return div().into_any_element();
        }
        let pending = input.is_some() && run.phase == superplexr_core::RunPhase::Pending;
        let collect =
            input.is_some() && run.outcome == Some(superplexr_core::FinishOutcome::Succeeded);
        let setup = self.verification_setup(mission_id, run_id);
        let preview = self
            .verification_preview
            .as_ref()
            .filter(|(mission, run, _)| *mission == mission_id && *run == run_id)
            .map(|(_, _, plan)| plan.clone());
        let action = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .px_2()
                .py_1()
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(RELAY))
                .text_color(rgb(RELAY))
                .cursor_pointer()
                .child(label)
        };
        div()
            .id("verification-panel")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(rgb(HAIRLINE))
            .rounded(px(4.0))
            .font_family(UI_FONT)
            .text_xs()
            .child(div().text_color(rgb(RELAY)).child("PROJECT CHECKS"))
            .child(
                div()
                    .text_color(rgb(SIGNAL))
                    .child("Trusted local execution · not a sandbox · no automatic acceptance"),
            )
            .child(review_fact(
                "FILES",
                format!(
                    "Plan: {}\nEvidence: {}\nRunner: {}",
                    setup.plan_path.display(),
                    setup.evidence_root.display(),
                    setup.runner_path.display()
                ),
                TRACE,
            ))
            .when(!self.review_action_busy, |panel| {
                panel.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            action("verification-plan", "Choose plan").on_click(cx.listener(
                                move |desktop, _, _, cx| {
                                    desktop.choose_verification_path(
                                        mission_id,
                                        run_id,
                                        Choice::Plan,
                                        cx,
                                    )
                                },
                            )),
                        )
                        .child(action("verification-evidence", "Evidence folder").on_click(
                            cx.listener(move |desktop, _, _, cx| {
                                desktop.choose_verification_path(
                                    mission_id,
                                    run_id,
                                    Choice::Evidence,
                                    cx,
                                )
                            }),
                        ))
                        .child(action("verification-runner", "Runner executable").on_click(
                            cx.listener(move |desktop, _, _, cx| {
                                desktop.choose_verification_path(
                                    mission_id,
                                    run_id,
                                    Choice::Runner,
                                    cx,
                                )
                            }),
                        ))
                        .when(source || pending, |actions| {
                            actions.child(
                                action("verification-review", "Review check plan").on_click(
                                    cx.listener(move |desktop, _, _, cx| {
                                        desktop.review_verification_plan(mission_id, run_id, cx)
                                    }),
                                ),
                            )
                        })
                        .when(collect, |actions| {
                            actions.child(
                                action("verification-collect", "Collect evidence").on_click(
                                    cx.listener(move |desktop, _, _, cx| {
                                        desktop.collect_verification(mission_id, run_id, cx)
                                    }),
                                ),
                            )
                        }),
                )
            })
            .when_some(preview, |panel, plan| {
                panel
                    .child(review_fact(
                        "REVIEWED COMMANDS",
                        format!("SHA-256 {}\n\n{}", plan.sha256, plan.description),
                        CHALK,
                    ))
                    .when(!self.review_action_busy && (source || pending), |panel| {
                        panel.child(
                            action("verification-launch", "Run reviewed checks").on_click(
                                cx.listener(move |desktop, _, window, cx| {
                                    desktop.run_reviewed_checks(
                                        mission_id,
                                        run_id,
                                        if pending {
                                            LaunchTarget::Verifier(run_id)
                                        } else {
                                            LaunchTarget::Subject(run_id)
                                        },
                                        window,
                                        cx,
                                    )
                                }),
                            ),
                        )
                    })
            })
            .into_any_element()
    }
}
