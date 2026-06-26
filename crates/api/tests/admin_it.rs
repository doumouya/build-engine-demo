//! Integration tests for the admin/registry read surface + memberships. `#[ignore]`d; run with:
//!   DATABASE_URL=postgres://.../build_engine_test cargo test -p api --test admin_it -- --ignored

use api::admin::{self, GrantBody};
use api::cases::{self, CreateCaseBody};
use api::AppError;
use sqlx::PgPool;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await.unwrap();
    api::run_migrations(&pool).await.unwrap();
    sqlx::query("insert into type_definitions (type_id,id_prefix,display_name) values ('user','USR','User') on conflict do nothing")
        .execute(&pool).await.unwrap();
    sqlx::query("insert into entities (id,type) values ('USR_ADMIN','user') on conflict do nothing")
        .execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
#[ignore]
async fn types_and_objects_are_listed() {
    let pool = pool().await;
    let detail = cases::create_case(
        &pool,
        CreateCaseBody { title: "admin object".into(), workflow_id: None, priority: None, assignee_id: None, scope_parent_id: None, actor_id: Some("USR_ADMIN".into()) },
        Some("USR_ADMIN"),
    )
    .await
    .unwrap();
    let case_id = detail.case.entity_id;

    let types = admin::list_types(&pool).await.unwrap();
    assert!(types.iter().any(|t| t.type_id == "case"), "the seeded case type is listed");

    let objects = admin::list_objects(&pool, Some("case"), 200).await.unwrap();
    assert!(objects.iter().any(|o| o.entity_id == case_id && o.r#type == "case"));
}

#[tokio::test]
#[ignore]
async fn workflows_are_readable() {
    let pool = pool().await;
    let all = admin::list_workflows(&pool).await.unwrap();
    let feature = all.iter().find(|w| w.workflow_id == "feature").expect("seeded feature workflow");
    assert_eq!(feature.initial, "backlog");
    assert_eq!(feature.states.as_array().unwrap().len(), 4); // backlog,in_progress,in_review,done

    let one = admin::get_workflow(&pool, "feature").await.unwrap();
    assert_eq!(one.close_checks.as_array().unwrap().len(), 3);

    assert!(matches!(
        admin::get_workflow(&pool, "nope").await.unwrap_err(),
        AppError::NotFound("unknown_workflow")
    ));
}

#[tokio::test]
#[ignore]
async fn memberships_grant_validate_and_revoke() {
    let pool = pool().await;
    let detail = cases::create_case(
        &pool,
        CreateCaseBody { title: "membership target".into(), workflow_id: None, priority: None, assignee_id: None, scope_parent_id: None, actor_id: Some("USR_ADMIN".into()) },
        Some("USR_ADMIN"),
    )
    .await
    .unwrap();
    let obj = detail.case.entity_id;

    let granted = admin::grant(&pool, GrantBody { object_id: obj.clone(), member_id: "USR_ADMIN".into(), role: "admin".into() }).await.unwrap();
    assert_eq!(granted.role, "admin");

    let mems = admin::list_memberships(&pool, Some(&obj), None).await.unwrap();
    assert!(mems.iter().any(|m| m.member_id == "USR_ADMIN" && m.role == "admin"));
    assert!(mems.iter().any(|m| m.role == "owner"), "the creator's owner membership is also present");

    // a bogus role is a 422; a bogus reference is a 400
    assert!(matches!(
        admin::grant(&pool, GrantBody { object_id: obj.clone(), member_id: "USR_ADMIN".into(), role: "king".into() }).await.unwrap_err(),
        AppError::Unprocessable { .. }
    ));
    assert!(matches!(
        admin::grant(&pool, GrantBody { object_id: "ENT_NOPE".into(), member_id: "USR_ADMIN".into(), role: "member".into() }).await.unwrap_err(),
        AppError::BadRequest { .. }
    ));

    let removed = admin::revoke(&pool, &obj, "USR_ADMIN", Some("admin")).await.unwrap();
    assert_eq!(removed, 1, "the admin membership was revoked");
}

#[tokio::test]
#[ignore]
async fn create_object_mints_generic_entities() {
    let pool = pool().await;
    // bootstrap a user with no actor — the write counterpart that lets a fresh DB mint actors
    let user = admin::create_object(
        &pool,
        admin::CreateObjectBody {
            type_: "user".into(),
            data: Some(serde_json::json!({ "name": "Zoe" })),
            scope_parent_id: None,
            actor_id: None,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(user.r#type, "user");
    assert!(user.entity_id.starts_with("USR_"));
    assert_eq!(user.data["name"], "Zoe");

    assert!(admin::list_objects(&pool, Some("user"), 200).await.unwrap().iter().any(|o| o.entity_id == user.entity_id));

    // a minted user can now be granted a membership (previously only cases could be entities)
    let target = cases::create_case(&pool, a_case("grant target"), Some("USR_ADMIN")).await.unwrap().case.entity_id;
    admin::grant(&pool, GrantBody { object_id: target, member_id: user.entity_id.clone(), role: "viewer".into() }).await.unwrap();

    // an unknown type is a clean 404
    assert!(matches!(
        admin::create_object(&pool, admin::CreateObjectBody { type_: "griffin".into(), data: None, scope_parent_id: None, actor_id: None }, None).await.unwrap_err(),
        AppError::NotFound("unknown_type")
    ));
}

fn a_case(title: &str) -> CreateCaseBody {
    CreateCaseBody { title: title.into(), workflow_id: None, priority: None, assignee_id: None, scope_parent_id: None, actor_id: Some("USR_ADMIN".into()) }
}
