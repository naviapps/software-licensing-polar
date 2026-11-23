use crate::{
    protocol::{API_VERSION, Decision, Environment, HttpResponse, Intent, request, respond},
    test_fixtures::{
        BENEFIT, CREATED, INSTANCE, ORGANIZATION, OTHER, activation, config, granted_response,
        intent,
    },
};
use serde_json::{Value, json};
use software_licensing::{
    Command, Effect, ErrorCode, Failure, MutationOutcome, Operation, Policy, ProviderReply, Record,
    Session, Status,
};

const NOW: f64 = CREATED + 100.0;

fn response(status: u16, value: Option<Value>) -> HttpResponse {
    HttpResponse {
        status_code: status,
        body: value.map_or_else(String::new, |v| v.to_string()),
        retry_after: None,
        api_version: Some(API_VERSION.into()),
    }
}
fn decision(operation: Operation, status: u16, value: Option<Value>) -> Decision {
    decide(intent(operation), response(status, value))
}
fn decide(intent: Intent, response: HttpResponse) -> Decision {
    let config = config();
    let request = request(&config, intent).unwrap();
    respond(&config, &request, response, 1, NOW)
}
fn complete(decision: Decision) -> Result<ProviderReply, Failure> {
    match decision {
        Decision::Complete(result) => result,
        Decision::Retry { .. } => panic!("Expected completion"),
    }
}
fn failure(decision: Decision) -> Failure {
    complete(decision).err().expect("Expected failure")
}
fn session_reply(decision: Decision) -> ProviderReply {
    complete(decision).unwrap_or_else(|failure| ProviderReply::Failed { failure })
}
fn persist(
    session: &mut Session,
    reply: software_licensing::Reply,
    now: f64,
) -> software_licensing::Reply {
    if let Some(Effect::Persist { id, .. }) = reply.effect {
        session
            .dispatch(Command::Persisted {
                id,
                success: true,
                now,
            })
            .unwrap()
    } else {
        reply
    }
}

#[test]
fn configuration_rejects_an_empty_license_scope() {
    let mut config = config();
    config.license_scope.benefit_ids.clear();
    assert_eq!(
        config.validated().unwrap_err().code,
        ErrorCode::InvalidProviderConfiguration
    );
}

#[test]
fn fixed_versioned_public_json_requests_do_not_send_tokens_or_usage() {
    for environment in [Environment::Production, Environment::Sandbox] {
        let mut cfg = config();
        cfg.environment = environment;
        cfg.activation_instance_name = " Fixture ".into();
        for operation in [
            Operation::Activate,
            Operation::Validate,
            Operation::Deactivate,
        ] {
            let request = request(&cfg, intent(operation)).unwrap();
            let base = if environment == Environment::Production {
                "https://api.polar.sh"
            } else {
                "https://sandbox-api.polar.sh"
            };
            let action = match operation {
                Operation::Activate => "activate",
                Operation::Validate => "validate",
                Operation::Deactivate => "deactivate",
            };
            assert_eq!(
                request.url,
                format!("{base}/v1/customer-portal/license-keys/{action}")
            );
            let body: Value = serde_json::from_str(&request.body).unwrap();
            assert_eq!(body["organization_id"], ORGANIZATION);
            assert_eq!(body["key"], "TEST-KEY");
            assert!(body.get("increment_usage").is_none());
            assert!(body.get("meta").is_none());
            if operation == Operation::Activate {
                assert_eq!(body["label"], "Fixture");
                assert!(body.get("activation_id").is_none());
            } else {
                assert_eq!(body["activation_id"], INSTANCE);
            }
            if operation == Operation::Validate {
                assert_eq!(body["benefit_id"], BENEFIT);
            }
        }
    }
    let mut input = intent(Operation::Activate);
    input.license_key = Some(" quote\"&=\\\n key ".into());
    let body: Value = serde_json::from_str(&request(&config(), input).unwrap().body).unwrap();
    assert_eq!(body["key"], "quote\"&=\\key");
}
#[test]
fn foreign_or_incomplete_activation_never_reaches_any_provider() {
    for operation in [Operation::Validate, Operation::Deactivate] {
        for field in 0..5 {
            let mut input = intent(operation);
            let a = input.activation.as_mut().unwrap();
            match field {
                0 => a.source = "fixture-provider".into(),
                1 => a.plan_identifier = OTHER.into(),
                2 => a.activation_identifier = None,
                3 => a.activation_identifier = Some("not-a-uuid".into()),
                _ => a.license_key = None,
            };
            assert!(request(&config(), input).is_err());
        }
    }
    for key in ["".to_owned(), "  \n".to_owned(), "x".repeat(4097)] {
        let mut input = intent(Operation::Activate);
        input.license_key = Some(key);
        assert_eq!(
            request(&config(), input).err().unwrap().code,
            ErrorCode::InvalidLicenseKey
        );
    }
}
#[test]
fn valid_activation_and_validation_preserve_benefit_and_optional_expiration() {
    for operation in [Operation::Activate, Operation::Validate] {
        for expires in [Value::Null, json!("2023-11-15T22:13:20.125Z")] {
            let mut value = granted_response(operation);
            let key = if operation == Operation::Activate {
                &mut value["license_key"]
            } else {
                &mut value
            };
            key["expires_at"] = expires.clone();
            match complete(decision(operation, 200, Some(value))).unwrap() {
                ProviderReply::Activated { activation } => {
                    assert_eq!(activation.source, "polar");
                    assert_eq!(activation.plan_identifier, BENEFIT);
                    assert_eq!(activation.activation_identifier.as_deref(), Some(INSTANCE));
                    assert_eq!(activation.activated_at, CREATED);
                    assert_eq!(
                        activation.expires_at,
                        if expires.is_null() {
                            None
                        } else {
                            Some(CREATED + 86400.125)
                        }
                    );
                }
                ProviderReply::Validated { validation } => {
                    assert!(validation.is_valid);
                    assert_eq!(validation.plan_identifier.as_deref(), Some(BENEFIT));
                    assert_eq!(
                        validation.expires_at,
                        if expires.is_null() {
                            None
                        } else {
                            Some(CREATED + 86400.125)
                        }
                    );
                }
                _ => panic!("Expected accepted key"),
            }
        }
    }
}
#[test]
fn scope_key_and_activation_identity_must_match_for_both_success_shapes() {
    for operation in [Operation::Activate, Operation::Validate] {
        let prefix = if operation == Operation::Activate {
            "/license_key"
        } else {
            ""
        };
        let activation_prefix = if operation == Operation::Activate {
            ""
        } else {
            "/activation"
        };
        for (pointer, replacement) in [
            (format!("{prefix}/organization_id"), json!(OTHER)),
            (format!("{prefix}/benefit_id"), json!(OTHER)),
            (format!("{prefix}/key"), json!("DIFFERENT-KEY")),
            (format!("{activation_prefix}/license_key_id"), json!(OTHER)),
        ] {
            let mut value = granted_response(operation);
            *value.pointer_mut(&pointer).unwrap() = replacement;
            let failure = failure(decision(operation, 200, Some(value)));
            assert_eq!(
                failure.code,
                if operation == Operation::Validate {
                    ErrorCode::UnexpectedProviderResponse
                } else {
                    ErrorCode::InvalidLicense
                }
            );
            assert_eq!(
                failure.mutation_outcome,
                if operation == Operation::Activate {
                    Some(MutationOutcome::Unknown)
                } else {
                    None
                }
            );
        }
        for pointer in [
            format!("{prefix}/id"),
            format!("{prefix}/organization_id"),
            format!("{prefix}/benefit_id"),
            format!("{activation_prefix}/id"),
        ] {
            let mut value = granted_response(operation);
            *value.pointer_mut(&pointer).unwrap() = json!("malformed");
            assert_eq!(
                failure(decision(operation, 200, Some(value))).code,
                ErrorCode::UnexpectedProviderResponse
            );
        }
    }
    let mut value = granted_response(Operation::Validate);
    value["activation"]["id"] = json!(OTHER);
    assert_eq!(
        failure(decision(Operation::Validate, 200, Some(value))).code,
        ErrorCode::UnexpectedProviderResponse
    );

    let mut configuration = config();
    configuration.license_scope.benefit_ids.push(OTHER.into());
    let mut value = granted_response(Operation::Validate);
    value["benefit_id"] = json!(OTHER);
    let request = request(&configuration, intent(Operation::Validate)).unwrap();
    let result = respond(&configuration, &request, response(200, Some(value)), 1, NOW);
    assert_eq!(failure(result).code, ErrorCode::UnexpectedProviderResponse);
}
#[test]
fn successful_response_requires_explicit_expiry_status_and_created_identity() {
    for operation in [Operation::Activate, Operation::Validate] {
        for missing in [true, false] {
            let mut value = granted_response(operation);
            let key = if operation == Operation::Activate {
                &mut value["license_key"]
            } else {
                &mut value
            };
            if missing {
                key.as_object_mut().unwrap().remove("expires_at");
            } else {
                key["expires_at"] = json!("bad-date");
            }
            assert_eq!(
                failure(decision(operation, 200, Some(value))).code,
                ErrorCode::UnexpectedProviderResponse
            );
        }
        for status in ["revoked", "disabled", "active", "GRANTED"] {
            let mut value = granted_response(operation);
            let key = if operation == Operation::Activate {
                &mut value["license_key"]
            } else {
                &mut value
            };
            key["status"] = json!(status);
            assert_eq!(
                failure(decision(operation, 200, Some(value))).code,
                if matches!(status, "revoked" | "disabled") {
                    ErrorCode::InvalidLicense
                } else {
                    ErrorCode::UnexpectedProviderResponse
                }
            );
        }
    }
    for created in [Value::Null, json!("invalid"), json!(1)] {
        let mut value = granted_response(Operation::Activate);
        value["created_at"] = created;
        assert_eq!(
            failure(decision(Operation::Activate, 200, Some(value))).code,
            ErrorCode::UnexpectedProviderResponse
        );
    }
}
#[test]
fn expired_in_flight_activation_retains_release_identity_and_validation_rejects_elapsed_key() {
    let expiry = CREATED + 50.0;
    let mut created = granted_response(Operation::Activate);
    created["license_key"]["expires_at"] = json!("2023-11-14T22:14:10Z");
    let ProviderReply::Activated { activation } =
        complete(decision(Operation::Activate, 200, Some(created))).unwrap()
    else {
        panic!("Need activation identity")
    };
    assert_eq!(activation.expires_at, Some(expiry));
    assert!(activation.expired(NOW));
    assert_eq!(activation.activation_identifier.as_deref(), Some(INSTANCE));
    let mut checked = granted_response(Operation::Validate);
    checked["expires_at"] = json!("2023-11-14T22:14:10Z");
    assert_eq!(
        failure(decision(Operation::Validate, 200, Some(checked))).code,
        ErrorCode::ExpiredLicense
    );
}
#[test]
fn only_versioned_empty_204_confirms_deactivation() {
    assert!(matches!(
        complete(decision(Operation::Deactivate, 204, None)),
        Ok(ProviderReply::Deactivated)
    ));
    for (status, value) in [
        (200, None),
        (200, Some(json!({"deactivated":true}))),
        (204, Some(json!({}))),
        (404, Some(json!({"error":"ResourceNotFound"}))),
    ] {
        let failed = failure(decision(Operation::Deactivate, status, value));
        assert_ne!(failed.code, ErrorCode::Cancelled);
    }
}
#[test]
fn missing_or_wrong_api_version_never_grants_access() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        for version in [None, Some("2025-01".into())] {
            let mut reply = response(
                if operation == Operation::Deactivate {
                    204
                } else {
                    200
                },
                (operation != Operation::Deactivate).then(|| granted_response(operation)),
            );
            reply.api_version = version;
            assert_eq!(
                failure(decide(intent(operation), reply)).code,
                ErrorCode::UnexpectedProviderResponse
            );
        }
    }
}
#[test]
fn typed_rejections_differ_from_generic_version_router_404_and_never_echo_provider_input() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        for (status, kind, version, expected) in [
            (
                404,
                "ResourceNotFound",
                Some(API_VERSION),
                ErrorCode::InvalidLicense,
            ),
            (404, "ResourceNotFound", None, ErrorCode::RequestFailure),
            (
                404,
                "ResourceNotFound",
                Some("2025-01"),
                ErrorCode::RequestFailure,
            ),
            (
                404,
                "Not Found",
                Some(API_VERSION),
                ErrorCode::RequestFailure,
            ),
            (
                403,
                "NotPermitted",
                Some(API_VERSION),
                ErrorCode::InvalidLicense,
            ),
            (
                422,
                "RequestValidationError",
                Some(API_VERSION),
                ErrorCode::RequestFailure,
            ),
            (
                400,
                "BadRequest",
                Some(API_VERSION),
                ErrorCode::RequestFailure,
            ),
        ] {
            let mut reply = response(
                status,
                Some(json!({"error":kind,"detail":"PRIVATE-KEY","input":"PRIVATE-KEY"})),
            );
            reply.api_version = version.map(str::to_owned);
            let failure = failure(decide(intent(operation), reply));
            assert_eq!(failure.code, expected);
            assert_eq!(failure.status_code, Some(status));
            assert!(
                !serde_json::to_string(&failure)
                    .unwrap()
                    .contains("PRIVATE-KEY")
            );
            if operation != Operation::Validate {
                let expected = if version == Some(API_VERSION) {
                    MutationOutcome::NotApplied
                } else {
                    MutationOutcome::Unknown
                };
                assert_eq!(failure.mutation_outcome, Some(expected));
            }
        }
    }
}
#[test]
fn retries_preserve_rate_limit_advice_and_never_replay_uncertain_mutation() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        let reply = decide(intent(operation), response(503, None));
        assert_eq!(
            matches!(reply, Decision::Retry { .. }),
            operation == Operation::Validate
        );
        if operation != Operation::Validate {
            assert_eq!(
                failure(reply).mutation_outcome,
                Some(MutationOutcome::Unknown)
            );
        }
        let mut limited = response(429, None);
        limited.retry_after = Some("2".into());
        let Decision::Retry { delay, rejection } = decide(intent(operation), limited) else {
            panic!("Expected rate-limit retry");
        };
        assert_eq!(delay, 2.0);
        assert_eq!(rejection.retry_after_seconds, Some(2));
        for after in [
            "9999999999999999999999999999999999999999999999999999999999",
            "Wed, 15 Nov 2023 00:00:00 GMT",
        ] {
            let mut limited = response(429, None);
            limited.retry_after = Some(after.into());
            assert_eq!(
                failure(decide(intent(operation), limited)).code,
                ErrorCode::RateLimited
            );
        }
    }
}
#[test]
fn malformed_json_success_and_redirect_mutations_remain_unknown() {
    for operation in [Operation::Activate, Operation::Deactivate] {
        for (status, body) in [(200, "{"), (302, ""), (204, "not empty")] {
            let mut reply = response(status, None);
            reply.body = body.into();
            assert_eq!(
                failure(decide(intent(operation), reply)).mutation_outcome,
                Some(MutationOutcome::Unknown)
            );
        }
    }
}
#[test]
fn untrusted_validation_failures_keep_only_the_saved_grants_bounded_grace() {
    let mut generic_missing = response(404, Some(json!({"detail": "Not Found"})));
    generic_missing.api_version = None;
    let mut other_activation = granted_response(Operation::Validate);
    other_activation["status"] = json!("revoked");
    other_activation["activation"]["id"] = json!(OTHER);
    let rejected = response(404, Some(json!({"error": "ResourceNotFound"})));

    let mut saved = activation();
    saved.expires_at = Some(NOW + 600.0);
    for (response, expected_error, expected_status) in [
        (
            generic_missing,
            ErrorCode::RequestFailure,
            Status::GracePeriod,
        ),
        (
            response(200, Some(other_activation)),
            ErrorCode::UnexpectedProviderResponse,
            Status::GracePeriod,
        ),
        (rejected, ErrorCode::InvalidLicense, Status::Invalid),
    ] {
        let record = Record {
            activation: Some(saved.clone()),
            status: Status::Active,
            last_validated_at: Some(CREATED),
            ..Record::default()
        };
        let mut session = Session::default();
        session
            .dispatch(Command::Restore {
                policy: Policy::default(),
                record: Some(serde_json::to_value(record).unwrap()),
                now: NOW,
            })
            .unwrap();
        let start = session
            .dispatch(Command::Refresh {
                only_if_due: false,
                now: NOW,
            })
            .unwrap();
        let Some(Effect::Provider {
            id,
            operation,
            license_key,
            activation,
        }) = start.effect
        else {
            panic!("Need validation")
        };
        let decision = decide(
            Intent {
                operation,
                license_key,
                activation,
            },
            response,
        );
        let reply = session
            .dispatch(Command::Provider {
                id,
                result: session_reply(decision),
                now: NOW,
            })
            .unwrap();
        let Some(Effect::Persist { record, .. }) = &reply.effect else {
            panic!("Need saved failure")
        };
        assert_eq!(record.activation.as_ref(), Some(&saved));
        assert_eq!(record.last_validated_at, Some(CREATED));
        assert_eq!(
            record.last_refresh_failure.as_ref().unwrap().code,
            expected_error
        );
        let grace_expiry = if expected_status == Status::GracePeriod {
            saved.expires_at
        } else {
            None
        };
        assert_eq!(record.grace_period_expires_at, grace_expiry);
        let state = persist(&mut session, reply, NOW).state;
        assert!(state.has_activation);
        assert_eq!(state.status, expected_status);
        assert_eq!(state.plan.expires_at, grace_expiry);
    }
}

#[test]
fn failed_deactivation_keeps_saved_instance_and_pending_release_after_restart() {
    for version in [None, Some(API_VERSION)] {
        let a = activation();
        let record = Record {
            activation: Some(a.clone()),
            status: Status::Active,
            last_validated_at: Some(CREATED),
            ..Record::default()
        };
        let mut session = Session::default();
        session
            .dispatch(Command::Restore {
                policy: Policy::default(),
                record: Some(serde_json::to_value(record).unwrap()),
                now: NOW,
            })
            .unwrap();
        let start = session.dispatch(Command::Deactivate { now: NOW }).unwrap();
        let start = persist(&mut session, start, NOW);
        let Some(Effect::Provider {
            id,
            operation,
            activation: saved,
            ..
        }) = start.effect
        else {
            panic!("Release must persist its identity before contacting the provider");
        };
        assert_eq!(operation, Operation::Deactivate);
        assert_eq!(saved.as_ref(), Some(&a));
        let mut reply = response(404, Some(json!({"error":"ResourceNotFound"})));
        reply.api_version = version.map(str::to_owned);
        let result = session_reply(decide(intent(operation), reply));
        let reply = session
            .dispatch(Command::Provider {
                id,
                result,
                now: NOW,
            })
            .unwrap();
        let Some(Effect::Persist { record, .. }) = &reply.effect else {
            panic!("Need durable failure");
        };
        assert!(record.pending_deactivation);
        assert_eq!(record.activation.as_ref(), Some(&a));
        let saved = serde_json::to_value(record).unwrap();
        let reply = persist(&mut session, reply, NOW);
        assert!(reply.state.has_pending_deactivation);
        assert!(!reply.state.plan.is_licensed);
        let mut restored = Session::default();
        let reply = restored
            .dispatch(Command::Restore {
                policy: Policy::default(),
                record: Some(saved),
                now: NOW,
            })
            .unwrap();
        assert!(reply.state.has_pending_deactivation);
        assert!(reply.state.has_activation);
    }
}

#[test]
fn uncertain_activation_stays_journaled_and_cannot_be_replayed_after_restart() {
    let mut malformed = response(200, None);
    malformed.body = "{".into();
    let mut generic_404 = response(404, None);
    generic_404.api_version = None;
    for reply in [malformed, generic_404] {
        assert_uncertain_activation_survives_reopen(reply);
    }
}

fn assert_uncertain_activation_survives_reopen(reply: HttpResponse) {
    let mut session = Session::default();
    session
        .dispatch(Command::Restore {
            policy: Policy::default(),
            record: None,
            now: NOW,
        })
        .unwrap();
    let start = session
        .dispatch(Command::Activate {
            license_key: Some("TEST-KEY".into()),
            now: NOW,
        })
        .unwrap();
    let start = persist(&mut session, start, NOW);
    let Some(Effect::Provider { id, .. }) = start.effect else {
        panic!("Expected activation effect");
    };
    let result = session_reply(decide(intent(Operation::Activate), reply));
    let reply = session
        .dispatch(Command::Provider {
            id,
            result,
            now: NOW,
        })
        .unwrap();
    let Some(Effect::Persist { record, .. }) = &reply.effect else {
        panic!("Need durable uncertainty");
    };
    assert!(record.pending_activation.is_some());
    let saved = serde_json::to_value(record).unwrap();
    let reply = persist(&mut session, reply, NOW);
    assert!(reply.state.has_pending_activation);
    assert_eq!(reply.error.unwrap().code, ErrorCode::MutationOutcomeUnknown);
    let mut restored = Session::default();
    let reply = restored
        .dispatch(Command::Restore {
            policy: Policy::default(),
            record: Some(saved),
            now: NOW,
        })
        .unwrap();
    assert!(reply.state.has_pending_activation);
    assert_eq!(
        restored
            .dispatch(Command::Activate {
                license_key: Some("TEST-KEY".into()),
                now: NOW
            })
            .err()
            .unwrap()
            .code,
        ErrorCode::ActivationPending
    );
}
