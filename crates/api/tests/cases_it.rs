//! Integration tests — they touch a real Postgres, so they are `#[ignore]`d and run explicitly:
//!   DATABASE_URL=postgres://.../build_engine_test cargo test -p api -- --ignored
//! Each test is self-contained (its own freshly-minted case id), so they share one database safely.

use api::cases::{self, CreateCaseBody};
use api::AppError;
use sqlx::PgPool;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    api::run_migrations(&pool).await.expect("migrate");
    ensure_actor(&pool, "USR_TEST").await;
    pool
}

/// Tests need a real entity to own/assign; seed a `user` type + a stable test user (idempotent).
async fn ensure_actor(pool: &PgPool, id: &str) {
    sqlx::query(
        "insert into type_definitions (type_id, id_prefix, display_name) values ('user','USR','User') \
         on conflict do nothing",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("insert into entities (id, type) values ($1, 'user') on conflict do nothing")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

fn new_case(title: &str) -> CreateCaseBody {
    CreateCaseBody {
        title: title.to_string(),
        workflow_id: None,
        priority: None,
        assignee_id: None,
        scope_parent_id: None,
        actor_id: Some("USR_TEST".to_string()),
    }
}

async fn status_of(pool: &PgPool, id: &str) -> String {
    let (s,): (String,) = sqlx::query_as("select status from cases where entity_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    s
}

#[tokio::test]
#[ignore]
async fn create_persists_atomically() {
    let pool = pool().await;
    let detail = cases::create_case(&pool, new_case("bootstrap the api"), Some("USR_TEST"))
        .await
        .unwrap();
    let id = detail.case.entity_id.clone();

    assert_eq!(detail.case.status, "backlog");
    assert_eq!(detail.close_checks.len(), 3, "three close-checks seeded unpassed");
    assert!(detail.close_checks.iter().all(|c| !c.passed));

    // entity + cases + owner membership all landed
    let (ent,): (bool,) = sqlx::query_as("select exists(select 1 from entities where id=$1 and type='case')")
        .bind(&id).fetch_one(&pool).await.unwrap();
    assert!(ent);
    let (owner,): (bool,) =
        sqlx::query_as("select exists(select 1 from memberships where object_id=$1 and member_id='USR_TEST' and role='owner')")
            .bind(&id).fetch_one(&pool).await.unwrap();
    assert!(owner, "creator owns the case");
}

#[tokio::test]
#[ignore]
async fn set_status_illegal_returns_422_and_does_not_write() {
    let pool = pool().await;
    let detail = cases::create_case(&pool, new_case("illegal skip"), Some("USR_TEST")).await.unwrap();
    let id = detail.case.entity_id;

    let err = cases::set_status(&pool, &id, "done", Some("USR_TEST")).await.unwrap_err();
    match err {
        AppError::Unprocessable { kind, .. } => assert_eq!(kind, "invalid_transition"),
        other => panic!("expected invalid_transition, got {other:?}"),
    }
    assert_eq!(status_of(&pool, &id).await, "backlog", "no write on reject");
}

#[tokio::test]
#[ignore]
async fn close_gate_end_to_end() {
    let pool = pool().await;
    let detail = cases::create_case(&pool, new_case("close gate"), Some("USR_TEST")).await.unwrap();
    let id = detail.case.entity_id;

    cases::set_status(&pool, &id, "in_progress", Some("USR_TEST")).await.unwrap();
    cases::set_status(&pool, &id, "in_review", Some("USR_TEST")).await.unwrap();

    // entering terminal with no checks passed → 422 listing all three missing
    let err = cases::set_status(&pool, &id, "done", Some("USR_TEST")).await.unwrap_err();
    match err {
        AppError::Unprocessable { kind, missing, .. } => {
            assert_eq!(kind, "close_preconditions_unmet");
            assert_eq!(missing.unwrap().len(), 3);
        }
        other => panic!("expected close_preconditions_unmet, got {other:?}"),
    }

    for check in ["docs-reconciled", "tests-green", "reviewer-approved"] {
        cases::set_close_check(&pool, &id, check, true, None, Some("USR_TEST")).await.unwrap();
    }
    // now it closes cleanly — the engine agrees, so the trigger never fires
    let row = cases::set_status(&pool, &id, "done", Some("USR_TEST")).await.unwrap();
    assert_eq!(row.status, "done");
}

#[tokio::test]
#[ignore]
async fn trigger_is_the_backstop() {
    let pool = pool().await;
    let detail = cases::create_case(&pool, new_case("backstop"), Some("USR_TEST")).await.unwrap();
    let id = detail.case.entity_id;

    // Bypass the engine: raw UPDATE straight to terminal with checks unmet. The trigger must RAISE.
    let raw = sqlx::query("update cases set status = 'done' where entity_id = $1")
        .bind(&id)
        .execute(&pool)
        .await;
    let app_err: AppError = raw.unwrap_err().into();
    assert!(matches!(app_err, AppError::WorkflowGuard(_)), "close-gate backstop fired");

    // And an unknown status raised by the same guard:
    let raw2 = sqlx::query("update cases set status = 'nope' where entity_id = $1")
        .bind(&id)
        .execute(&pool)
        .await;
    let app_err2: AppError = raw2.unwrap_err().into();
    assert!(matches!(app_err2, AppError::WorkflowGuard(_)), "unknown-status backstop fired");

    assert_eq!(status_of(&pool, &id).await, "backlog", "no bypass write survived");
}

#[tokio::test]
#[ignore]
async fn not_found_is_leak_free() {
    let pool = pool().await;
    let err = cases::get_case(&pool, "CAS_DOESNOTEXIST", None).await.unwrap_err();
    assert!(matches!(err, AppError::NotFound("not_found")));
    let err2 = cases::set_status(&pool, "CAS_DOESNOTEXIST", "in_progress", None).await.unwrap_err();
    assert!(matches!(err2, AppError::NotFound("not_found")));
}

#[tokio::test]
#[ignore]
async fn same_state_redrop_is_noop_no_event() {
    let pool = pool().await;
    let detail = cases::create_case(&pool, new_case("redrop"), Some("USR_TEST")).await.unwrap();
    let id = detail.case.entity_id;

    let row = cases::set_status(&pool, &id, "backlog", Some("USR_TEST")).await.unwrap();
    assert_eq!(row.status, "backlog");

    let (n,): (i64,) =
        sqlx::query_as("select count(*) from events where entity_id=$1 and kind='case_status'")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0, "a same-column re-drop writes no status event");
}
