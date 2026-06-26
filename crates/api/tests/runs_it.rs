//! Integration tests for the orchestrator-runs surface. `#[ignore]`d; run with:
//!   DATABASE_URL=postgres://.../build_engine_test cargo test -p api --test runs_it -- --ignored

use std::time::Duration;

use api::runs::{self, HandoffBody, StartRunBody};
use api::{admin, AppError};
use sqlx::PgPool;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await.unwrap();
    api::run_migrations(&pool).await.unwrap();
    pool
}

fn handoff(role: &str) -> HandoffBody {
    HandoffBody {
        role: role.to_string(),
        gate: Some("ci".into()),
        outcome: Some("pass".into()),
        kind: None,
        attempt: None,
        retries: None,
        hops: None,
        note: None,
        actor_id: Some("USR_ORCH".into()),
    }
}

#[tokio::test]
#[ignore]
async fn run_lifecycle_records_handoffs_and_an_event() {
    let pool = pool().await;
    let detail = runs::start_run(
        &pool,
        StartRunBody { title: "build a feature".into(), case_id: None, actor_id: Some("USR_ORCH".into()) },
        Some("USR_ORCH"),
    )
    .await
    .unwrap();
    let id = detail.run.id.clone();
    assert!(id.starts_with("RUN_"));
    assert_eq!(detail.run.phase, "spec");
    assert_eq!(detail.run.status, "active");

    for role in ["architect", "coder", "tester", "reviewer", "ops"] {
        runs::record_handoff(&pool, &id, handoff(role), Some("USR_ORCH")).await.unwrap();
    }

    // an unknown role is a clean 422, not a DB 500
    let bad = runs::record_handoff(&pool, &id, handoff("wizard"), None).await.unwrap_err();
    assert!(matches!(bad, AppError::Unprocessable { kind, .. } if kind == "invalid_role"));

    // a handoff on a missing run is a leak-free 404
    let missing = runs::record_handoff(&pool, "RUN_DOESNOTEXIST", handoff("ops"), None).await.unwrap_err();
    assert!(matches!(missing, AppError::NotFound(_)));

    let finished = runs::update_run(&pool, &id, Some("done"), Some("done"), None).await.unwrap();
    assert_eq!(finished.status, "done");
    assert_eq!(finished.phase, "done");

    let got = runs::get_run(&pool, &id).await.unwrap();
    assert_eq!(got.handoffs.len(), 5, "five role handoffs recorded in order");
    assert_eq!(got.handoffs[0].role, "architect");
    assert_eq!(got.handoffs[0].kind, "gate"); // defaulted

    assert!(runs::list_runs(&pool, Some("done"), 200).await.unwrap().iter().any(|r| r.id == id));

    // the run_start event landed on the Monitor feed (fire-and-forget, so poll briefly)
    let mut seen = false;
    for _ in 0..20 {
        if !admin::list_events(&pool, Some(&id), Some("run_start"), 5).await.unwrap().is_empty() {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen, "a run_start event reached the activity feed");
}
