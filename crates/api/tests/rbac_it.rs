//! RBAC reach integration tests — `#[ignore]`d, run with a real Postgres:
//!   DATABASE_URL=postgres://.../build_engine_test cargo test -p api --test rbac_it -- --ignored
//!
//! Graph: ORG_RBAC → PRJ_RBAC → (a case). Alice is a member of the org, Bob of the project, Carol of
//! nothing. Reach descends, so Alice and Bob reach the case but Bob must not climb to the org, and
//! Carol reaches nothing.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use api::cases::{self, CreateCaseBody, ListQuery};
use api::{reach, AppError};
use sqlx::PgPool;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    api::run_migrations(&pool).await.expect("migrate");
    seed_graph(&pool).await;
    pool
}

async fn seed_type(pool: &PgPool, type_id: &str, prefix: &str) {
    sqlx::query("insert into type_definitions (type_id, id_prefix, display_name) values ($1, $2, $1) on conflict do nothing")
        .bind(type_id).bind(prefix).execute(pool).await.unwrap();
}

async fn make_entity(pool: &PgPool, id: &str, type_id: &str, parent: Option<&str>) {
    sqlx::query("insert into entities (id, type) values ($1, $2) on conflict do nothing")
        .bind(id).bind(type_id).execute(pool).await.unwrap();
    sqlx::query(
        "insert into entity_data (entity_id, type_id, scope_parent_id) values ($1, $2, $3) \
         on conflict (entity_id) do update set scope_parent_id = excluded.scope_parent_id",
    )
    .bind(id).bind(type_id).bind(parent).execute(pool).await.unwrap();
}

async fn add_member(pool: &PgPool, object: &str, member: &str) {
    sqlx::query("insert into memberships (object_id, member_id, role) values ($1, $2, 'member') on conflict do nothing")
        .bind(object).bind(member).execute(pool).await.unwrap();
}

/// Idempotent org graph + three users with distinct reach.
async fn seed_graph(pool: &PgPool) {
    for (t, p) in [("user", "USR"), ("org", "ORG"), ("project", "PRJ")] {
        seed_type(pool, t, p).await;
    }
    make_entity(pool, "ORG_RBAC", "org", None).await;
    make_entity(pool, "PRJ_RBAC", "project", Some("ORG_RBAC")).await;
    for u in ["USR_ALICE", "USR_BOB", "USR_CAROL"] {
        make_entity(pool, u, "user", None).await;
    }
    add_member(pool, "ORG_RBAC", "USR_ALICE").await; // org-level
    add_member(pool, "PRJ_RBAC", "USR_BOB").await; // project-level
    // Carol: deliberately no membership.
}

async fn reach_set(pool: &PgPool, actor: &str) -> BTreeSet<String> {
    reach::reachable_ids(pool, actor).await.unwrap().into_iter().collect()
}

/// Create a case scoped under the project, by Alice.
async fn make_case(pool: &PgPool) -> String {
    let body = CreateCaseBody {
        title: "rbac scoped case".into(),
        workflow_id: None,
        priority: None,
        assignee_id: None,
        scope_parent_id: Some("PRJ_RBAC".into()),
        actor_id: Some("USR_ALICE".into()),
    };
    cases::create_case(pool, body, Some("USR_ALICE")).await.unwrap().case.entity_id
}

#[tokio::test]
#[ignore]
async fn reach_descends_and_does_not_climb() {
    let pool = pool().await;
    let case = make_case(&pool).await;

    let alice = reach_set(&pool, "USR_ALICE").await;
    assert!(alice.contains("ORG_RBAC") && alice.contains("PRJ_RBAC") && alice.contains(case.as_str()));

    let bob = reach_set(&pool, "USR_BOB").await;
    assert!(bob.contains("PRJ_RBAC") && bob.contains(case.as_str()));
    assert!(!bob.contains("ORG_RBAC"), "membership on a child must NOT climb to the parent");

    let carol = reach_set(&pool, "USR_CAROL").await;
    assert!(!carol.contains(case.as_str()), "a non-member reaches nothing scoped here");

    assert!(reach::can_reach(&pool, "USR_ALICE", &case).await.unwrap());
    assert!(!reach::can_reach(&pool, "USR_CAROL", &case).await.unwrap());
}

#[tokio::test]
#[ignore]
async fn reads_are_leak_free_and_lists_are_filtered() {
    let pool = pool().await;
    let case = make_case(&pool).await;

    // An unreachable case is denied exactly like an absent one.
    assert!(matches!(
        cases::get_case(&pool, &case, Some("USR_CAROL")).await.unwrap_err(),
        AppError::NotFound("not_found")
    ));
    assert!(cases::get_case(&pool, &case, Some("USR_ALICE")).await.is_ok());
    assert!(cases::get_case(&pool, &case, None).await.is_ok(), "no actor ⇒ permissive");

    let carol_list = cases::list_cases(&pool, ListQuery { actor_id: Some("USR_CAROL".into()), ..Default::default() }).await.unwrap();
    assert!(!carol_list.items.iter().any(|i| i.entity_id == case));

    let bob_list = cases::list_cases(&pool, ListQuery { actor_id: Some("USR_BOB".into()), ..Default::default() }).await.unwrap();
    assert!(bob_list.items.iter().any(|i| i.entity_id == case));
}

/// The pure `engine::reachable` and the SQL recursive CTE must compute the identical set over the
/// same graph — the resolver's analogue of the engine↔trigger agreement proof.
#[tokio::test]
#[ignore]
async fn engine_resolver_agrees_with_the_sql_cte() {
    let pool = pool().await;
    let _case = make_case(&pool).await;

    // Load the whole scope graph + Alice's direct memberships, run the pure resolver, compare.
    let edges: Vec<(String, Option<String>)> =
        sqlx::query_as("select entity_id, scope_parent_id from entity_data")
            .fetch_all(&pool).await.unwrap();
    let mut children_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (child, parent) in edges {
        if let Some(p) = parent {
            children_of.entry(p).or_default().push(child);
        }
    }
    let seed: BTreeSet<String> =
        sqlx::query_as::<_, (String,)>("select object_id from memberships where member_id = 'USR_ALICE'")
            .fetch_all(&pool).await.unwrap()
            .into_iter().map(|(id,)| id).collect();

    let from_engine = engine::reachable(&seed, &children_of);
    let from_sql = reach_set(&pool, "USR_ALICE").await;
    assert_eq!(from_engine, from_sql, "pure engine resolver and SQL CTE must agree");
}
