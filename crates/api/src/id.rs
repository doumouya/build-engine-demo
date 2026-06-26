//! Entity id minting. Ids are `<PREFIX>_<32 hex>` (e.g. `CAS_AB12...`), where the suffix is a v4
//! UUID's 122 bits of randomness rendered uppercase. That is far beyond any practical collision
//! risk on a primary key, so there is deliberately no collision-retry loop. The prefix makes
//! `kind(id)` a cheap lookup against `type_definitions.id_prefix`.

pub fn new_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
    format!("{prefix}_{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = new_id("CAS");
        let b = new_id("CAS");
        assert!(a.starts_with("CAS_"));
        assert_eq!(a.len(), "CAS_".len() + 32);
        assert_ne!(a, b);
        assert!(a[4..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    }
}
