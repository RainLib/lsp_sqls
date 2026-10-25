pub mod clickhouse;
pub(crate) mod common;
pub mod elasticsearch_dsl;
pub mod elasticsearch_eql;
pub mod hive;
pub mod mongodb;
pub mod mysql;
pub mod postgres;
pub mod redis;
pub mod sqlite;

pub use clickhouse::ClickHouseDialect;
pub use elasticsearch_dsl::ElasticsearchDslDialect;
pub use elasticsearch_eql::ElasticsearchEqlDialect;
pub use hive::HiveDialect;
pub use mongodb::MongoDbDialect;
pub use mysql::MysqlDialect;
pub use postgres::PostgresDialect;
pub use redis::RedisDialect;
pub use sqlite::SqliteDialect;

use crate::dialect::Dialect;
use std::collections::HashMap;
use std::sync::Arc;

pub const MYSQL_COMPATIBILITY_ALIASES: &[&str] = &[
    "databend",
    "doris",
    "gbase",
    "goldendb",
    "manticoresearch",
    "starrocks",
    "sundb",
    "tidb",
];

pub const POSTGRES_COMPATIBILITY_ALIASES: &[&str] = &[
    "access",
    "cockroachdb",
    "dameng",
    "db2",
    "exasol",
    "firebird",
    "gaussdb",
    "greenplum",
    "h2",
    "highgo",
    "informix",
    "iris",
    "kingbase",
    "kwdb",
    "oceanbase-oracle",
    "opengauss",
    "oracle",
    "questdb",
    "redshift",
    "timescaledb",
    "vastbase",
    "vertica",
    "xugu",
    "yashandb",
    "yugabytedb",
];

pub const SQLITE_COMPATIBILITY_ALIASES: &[&str] = &["cloudflare-d1", "turso"];

pub const HIVE_COMPATIBILITY_ALIASES: &[&str] = &["argo"];

/// 方言注册表
pub struct DialectRegistry {
    dialects: Vec<Arc<dyn Dialect>>,
    aliases: HashMap<String, Arc<dyn Dialect>>,
}

impl DialectRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            dialects: Vec::new(),
            aliases: HashMap::new(),
        };

        // 注册所有方言
        let mysql = Arc::new(MysqlDialect::new());
        registry.register(mysql.clone());
        registry.register_alias("mariadb", mysql.clone());
        for alias in MYSQL_COMPATIBILITY_ALIASES {
            registry.register_alias(alias, mysql.clone());
        }

        let postgres = Arc::new(PostgresDialect::new());
        registry.register(postgres.clone());
        registry.register_alias("postgresql", postgres.clone());
        registry.register_alias("pgsql", postgres.clone());
        registry.register_alias("psql", postgres.clone());
        for alias in POSTGRES_COMPATIBILITY_ALIASES {
            registry.register_alias(alias, postgres.clone());
        }

        let sqlserver = Arc::new(PostgresDialect::sqlserver());
        registry.register(sqlserver.clone());
        registry.register_alias("mssql", sqlserver.clone());
        registry.register_alias("tsql", sqlserver);

        let sqlite = Arc::new(SqliteDialect::new());
        registry.register(sqlite.clone());
        registry.register_alias("sqlite3", sqlite.clone());
        // DuckDB shares the SQLite-compatible completion baseline for now.
        // Keep its public language ID distinct so external driver packages can
        // describe their real engine without being reduced to generic SQL.
        registry.register(Arc::new(SqliteDialect::ansi()));
        for alias in SQLITE_COMPATIBILITY_ALIASES {
            registry.register_alias(alias, sqlite.clone());
        }

        let hive = Arc::new(HiveDialect::new());
        registry.register(hive.clone());
        registry.register_alias("hql", hive.clone());
        for alias in HIVE_COMPATIBILITY_ALIASES {
            registry.register_alias(alias, hive.clone());
        }

        let elasticsearch_eql = Arc::new(ElasticsearchEqlDialect::new());
        registry.register(elasticsearch_eql.clone());
        registry.register_alias("eql", elasticsearch_eql.clone());
        registry.register_alias("es-eql", elasticsearch_eql);

        let elasticsearch_dsl = Arc::new(ElasticsearchDslDialect::new());
        registry.register(elasticsearch_dsl.clone());
        registry.register_alias("elasticsearch", elasticsearch_dsl.clone());
        registry.register_alias("elastic", elasticsearch_dsl.clone());
        registry.register_alias("es", elasticsearch_dsl.clone());
        registry.register_alias("es-dsl", elasticsearch_dsl);

        let clickhouse = Arc::new(ClickHouseDialect::new());
        registry.register(clickhouse.clone());
        registry.register_alias("ch", clickhouse);

        registry.register(Arc::new(RedisDialect::new()));
        let mongodb = Arc::new(MongoDbDialect::new());
        registry.register(mongodb.clone());
        registry.register_alias("mongo", mongodb.clone());
        registry.register_alias("json", mongodb);

        registry
    }

    pub fn register(&mut self, dialect: Arc<dyn Dialect>) {
        self.dialects.push(dialect);
    }

    pub fn register_alias(&mut self, alias: &str, dialect: Arc<dyn Dialect>) {
        self.aliases.insert(alias.to_lowercase(), dialect);
    }

    pub fn get_by_name(&self, name: &str) -> Option<Arc<dyn Dialect>> {
        if let Some(dialect) = self.aliases.get(&name.to_lowercase()) {
            return Some(dialect.clone());
        }

        self.dialects
            .iter()
            .find(|d| d.name().eq_ignore_ascii_case(name))
            .cloned()
    }

    /// Snapshot requests use an explicit, normalized registry identity.
    /// Preserve the resolved adapter and its registered identity rather than
    /// guessing a parser from the name of a related engine.
    pub fn get_explicit_profile(&self, name: &str) -> Option<Arc<dyn Dialect>> {
        if name.is_empty()
            || name.len() > 64
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
        {
            return None;
        }
        self.get_by_name(name)
    }

    pub fn list_names(&self) -> Vec<String> {
        self.dialects.iter().map(|d| d.name().to_string()).collect()
    }
}

impl Default for DialectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::DialectRegistry;
    #[test]
    fn explicit_profiles_preserve_registered_adapters_and_reject_unnormalized_ids() {
        let registry = DialectRegistry::new();
        for name in ["duckdb", "mariadb", "postgresql", "mssql", "argo", "turso"] {
            assert!(std::sync::Arc::ptr_eq(
                &registry.get_explicit_profile(name).unwrap(),
                &registry.get_by_name(name).unwrap()
            ));
        }
        assert!(!std::sync::Arc::ptr_eq(
            &registry.get_explicit_profile("duckdb").unwrap(),
            &registry.get_explicit_profile("sqlite").unwrap()
        ));
        for name in ["", "POSTGRES", "postgres ", "not-registered", "../sqlite"] {
            assert!(registry.get_explicit_profile(name).is_none());
        }
    }

    #[test]
    fn duckdb_keeps_its_own_identity_on_the_ansi_completion_baseline() {
        let registry = DialectRegistry::new();
        let dialect = registry
            .get_by_name("duckdb")
            .expect("DuckDB profile should be registered");

        assert_eq!(dialect.name(), "duckdb");
    }

    #[test]
    fn resolves_argo_to_the_hive_compatible_baseline() {
        let registry = DialectRegistry::new();
        let dialect = registry
            .get_by_name("argo")
            .expect("ArgoDB alias should be registered");

        assert_eq!(dialect.name(), "hive");
    }
}
