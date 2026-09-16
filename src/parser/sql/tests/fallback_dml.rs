use super::*;

#[test]
fn fallback_completion_context_handles_dml_table_and_column_positions() {
    let parser = SqlParser::new();

    let insert_table_sql = "INSERT INTO app.us";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_table_sql,
            position_at_end(insert_table_sql)
        ),
        CompletionContext::FromClause
    );

    let insert_action_sql = "INSERT INTO app.users ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_action_sql,
            position_at_end(insert_action_sql)
        ),
        CompletionContext::InsertActionClause
    );

    let insert_action_prefix_sql = "INSERT INTO app.users VAL";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_action_prefix_sql,
            position_at_end(insert_action_prefix_sql)
        ),
        CompletionContext::InsertActionClause
    );

    let insert_column_sql = "INSERT INTO app.users (";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_column_sql,
            position_at_end(insert_column_sql)
        ),
        CompletionContext::SelectClause
    );

    let insert_value_sql = "INSERT INTO app.users (name) VALUES (";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_value_sql,
            position_at_end(insert_value_sql)
        ),
        CompletionContext::InsertValueClause
    );

    let insert_value_prefix_sql = "INSERT INTO app.users (name) VALUES (NU";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_value_prefix_sql,
            position_at_end(insert_value_prefix_sql)
        ),
        CompletionContext::InsertValueClause
    );

    let insert_value_after_comma_sql = "INSERT INTO app.users (owner, name) VALUES ('app', ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_value_after_comma_sql,
            position_at_end(insert_value_after_comma_sql)
        ),
        CompletionContext::InsertValueClause
    );

    let insert_continuation_sql = "INSERT INTO app.users (name) VALUES ('app') ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_continuation_sql,
            position_at_end(insert_continuation_sql)
        ),
        CompletionContext::InsertContinuationClause
    );

    let insert_continuation_prefix_sql = "INSERT INTO app.users (name) VALUES ('app') O";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_continuation_prefix_sql,
            position_at_end(insert_continuation_prefix_sql)
        ),
        CompletionContext::InsertContinuationClause
    );

    let insert_default_continuation_sql = "INSERT INTO app.users DEFAULT VALUES ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_default_continuation_sql,
            position_at_end(insert_default_continuation_sql)
        ),
        CompletionContext::InsertContinuationClause
    );

    let insert_set_sql = "INSERT INTO app.users SET ";
    assert_eq!(
        parser.analyze_completion_context_fallback(insert_set_sql, position_at_end(insert_set_sql)),
        CompletionContext::WhereClause
    );

    let insert_set_operator_sql = "INSERT INTO app.users SET name ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_set_operator_sql,
            position_at_end(insert_set_operator_sql)
        ),
        CompletionContext::WhereClause
    );

    let insert_set_value_sql = "INSERT INTO app.users SET name = ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_set_value_sql,
            position_at_end(insert_set_value_sql)
        ),
        CompletionContext::ExpressionValueClause
    );

    let insert_set_continuation_sql = "INSERT INTO app.users SET name = 'app' ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_set_continuation_sql,
            position_at_end(insert_set_continuation_sql)
        ),
        CompletionContext::PredicateContinuationClause
    );

    let insert_duplicate_update_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON DUPLICATE KEY UPDATE ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_duplicate_update_sql,
            position_at_end(insert_duplicate_update_sql)
        ),
        CompletionContext::WhereClause
    );

    let insert_duplicate_update_operator_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON DUPLICATE KEY UPDATE name ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_duplicate_update_operator_sql,
            position_at_end(insert_duplicate_update_operator_sql)
        ),
        CompletionContext::WhereClause
    );

    let insert_duplicate_update_value_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON DUPLICATE KEY UPDATE name = ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_duplicate_update_value_sql,
            position_at_end(insert_duplicate_update_value_sql)
        ),
        CompletionContext::ExpressionValueClause
    );

    let insert_duplicate_update_continuation_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON DUPLICATE KEY UPDATE name = 'app' ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_duplicate_update_continuation_sql,
            position_at_end(insert_duplicate_update_continuation_sql)
        ),
        CompletionContext::PredicateContinuationClause
    );

    let insert_conflict_action_sql = "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_action_sql,
            position_at_end(insert_conflict_action_sql)
        ),
        CompletionContext::InsertConflictActionClause
    );

    let insert_conflict_target_sql = "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT (";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_target_sql,
            position_at_end(insert_conflict_target_sql)
        ),
        CompletionContext::InsertConflictTargetClause
    );

    let insert_conflict_target_action_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT (name) ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_target_action_sql,
            position_at_end(insert_conflict_target_action_sql)
        ),
        CompletionContext::InsertConflictActionClause
    );

    let insert_conflict_do_sql = "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT DO ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_do_sql,
            position_at_end(insert_conflict_do_sql)
        ),
        CompletionContext::InsertConflictActionClause
    );

    let insert_conflict_do_prefix_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT DO N";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_do_prefix_sql,
            position_at_end(insert_conflict_do_prefix_sql)
        ),
        CompletionContext::InsertConflictActionClause
    );

    let insert_conflict_constraint_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT ON CONSTRAINT ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_constraint_sql,
            position_at_end(insert_conflict_constraint_sql)
        ),
        CompletionContext::InsertConflictConstraintClause
    );

    let insert_conflict_constraint_prefix_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT ON CONSTRAINT users";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_constraint_prefix_sql,
            position_at_end(insert_conflict_constraint_prefix_sql)
        ),
        CompletionContext::InsertConflictConstraintClause
    );

    let insert_conflict_constraint_action_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT ON CONSTRAINT users_pkey ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_constraint_action_sql,
            position_at_end(insert_conflict_constraint_action_sql)
        ),
        CompletionContext::InsertConflictActionClause
    );

    let insert_conflict_update_set_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT (name) DO UPDATE SET ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_update_set_sql,
            position_at_end(insert_conflict_update_set_sql)
        ),
        CompletionContext::WhereClause
    );

    let insert_conflict_update_set_operator_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT (name) DO UPDATE SET name ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_update_set_operator_sql,
            position_at_end(insert_conflict_update_set_operator_sql)
        ),
        CompletionContext::WhereClause
    );

    let insert_conflict_update_set_value_sql =
        "INSERT INTO app.users (name) VALUES ('app') ON CONFLICT (name) DO UPDATE SET name = ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            insert_conflict_update_set_value_sql,
            position_at_end(insert_conflict_update_set_value_sql)
        ),
        CompletionContext::ExpressionValueClause
    );

    let update_value_sql = "UPDATE app.users SET name = ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_value_sql,
            position_at_end(update_value_sql)
        ),
        CompletionContext::ExpressionValueClause
    );

    let update_value_prefix_sql = "UPDATE app.users SET name = NU";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_value_prefix_sql,
            position_at_end(update_value_prefix_sql)
        ),
        CompletionContext::ExpressionValueClause
    );

    let update_value_continuation_sql = "UPDATE app.users SET name = 'app' ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_value_continuation_sql,
            position_at_end(update_value_continuation_sql)
        ),
        CompletionContext::PredicateContinuationClause
    );

    let update_next_assignment_sql = "UPDATE app.users SET name = 'app', ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_next_assignment_sql,
            position_at_end(update_next_assignment_sql)
        ),
        CompletionContext::WhereClause
    );

    let update_table_sql = "UPDATE app.us";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_table_sql,
            position_at_end(update_table_sql)
        ),
        CompletionContext::FromClause
    );

    let update_action_sql = "UPDATE app.users ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_action_sql,
            position_at_end(update_action_sql)
        ),
        CompletionContext::UpdateActionClause
    );

    let update_action_prefix_sql = "UPDATE app.users S";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            update_action_prefix_sql,
            position_at_end(update_action_prefix_sql)
        ),
        CompletionContext::UpdateActionClause
    );

    let update_set_sql = "UPDATE app.users SET ";
    assert_eq!(
        parser.analyze_completion_context_fallback(update_set_sql, position_at_end(update_set_sql)),
        CompletionContext::WhereClause
    );

    let delete_table_sql = "DELETE FROM app.se";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            delete_table_sql,
            position_at_end(delete_table_sql)
        ),
        CompletionContext::FromClause
    );

    let delete_action_sql = "DELETE FROM app.sessions ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            delete_action_sql,
            position_at_end(delete_action_sql)
        ),
        CompletionContext::DeleteActionClause
    );

    let delete_action_prefix_sql = "DELETE FROM app.sessions WH";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            delete_action_prefix_sql,
            position_at_end(delete_action_prefix_sql)
        ),
        CompletionContext::DeleteActionClause
    );

    let create_index_table_sql = "CREATE INDEX users_email_idx ON app.us";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            create_index_table_sql,
            position_at_end(create_index_table_sql)
        ),
        CompletionContext::FromClause
    );

    let create_index_column_sql = "CREATE INDEX users_email_idx ON app.users (";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            create_index_column_sql,
            position_at_end(create_index_column_sql)
        ),
        CompletionContext::SelectClause
    );

    let references_table_sql = "CREATE TABLE app.orders (user_id INT REFERENCES ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_table_sql,
            position_at_end(references_table_sql)
        ),
        CompletionContext::FromClause
    );

    let references_table_prefix_sql = "CREATE TABLE app.orders (user_id INT REFERENCES app.us";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_table_prefix_sql,
            position_at_end(references_table_prefix_sql)
        ),
        CompletionContext::FromClause
    );

    let references_column_sql = "CREATE TABLE app.orders (user_id INT REFERENCES app.users (";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_column_sql,
            position_at_end(references_column_sql)
        ),
        CompletionContext::ReferenceColumnClause
    );

    let references_column_prefix_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users (id";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_column_prefix_sql,
            position_at_end(references_column_prefix_sql)
        ),
        CompletionContext::ReferenceColumnClause
    );

    let references_completed_table_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_completed_table_sql,
            position_at_end(references_completed_table_sql)
        ),
        CompletionContext::ReferenceActionClause
    );

    let references_action_prefix_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users O";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_action_prefix_sql,
            position_at_end(references_action_prefix_sql)
        ),
        CompletionContext::ReferenceActionClause
    );

    let references_on_delete_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users ON DELETE ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_on_delete_sql,
            position_at_end(references_on_delete_sql)
        ),
        CompletionContext::ReferenceRuleClause
    );

    let references_on_update_prefix_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users ON UPDATE C";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            references_on_update_prefix_sql,
            position_at_end(references_on_update_prefix_sql)
        ),
        CompletionContext::ReferenceRuleClause
    );

    let references_on_delete_rule_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users ON DELETE CASCADE ";
    assert_ne!(
        parser.analyze_completion_context_fallback(
            references_on_delete_rule_sql,
            position_at_end(references_on_delete_rule_sql)
        ),
        CompletionContext::ReferenceRuleClause
    );

    let references_completed_column_sql =
        "CREATE TABLE app.orders (user_id INT REFERENCES app.users (id)";
    assert_ne!(
        parser.analyze_completion_context_fallback(
            references_completed_column_sql,
            position_at_end(references_completed_column_sql)
        ),
        CompletionContext::ReferenceColumnClause
    );

    let drop_index_sql = "DROP INDEX ";
    assert_eq!(
        parser.analyze_completion_context_fallback(drop_index_sql, position_at_end(drop_index_sql)),
        CompletionContext::IndexTargetClause
    );

    let drop_index_prefix_sql = "DROP INDEX users_email";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            drop_index_prefix_sql,
            position_at_end(drop_index_prefix_sql)
        ),
        CompletionContext::IndexTargetClause
    );

    let alter_drop_column_sql = "ALTER TABLE app.users DROP COLUMN ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_drop_column_sql,
            position_at_end(alter_drop_column_sql)
        ),
        CompletionContext::ColumnTargetClause
    );

    let alter_modify_column_sql = "ALTER TABLE app.users MODIFY COLUMN ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_modify_column_sql,
            position_at_end(alter_modify_column_sql)
        ),
        CompletionContext::ColumnTargetClause
    );

    let alter_modify_column_prefix_sql = "ALTER TABLE app.users MODIFY ow";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_modify_column_prefix_sql,
            position_at_end(alter_modify_column_prefix_sql)
        ),
        CompletionContext::ColumnTargetClause
    );

    let alter_change_column_sql = "ALTER TABLE app.users CHANGE COLUMN ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_change_column_sql,
            position_at_end(alter_change_column_sql)
        ),
        CompletionContext::ColumnTargetClause
    );

    let alter_change_column_prefix_sql = "ALTER TABLE app.users CHANGE ow";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_change_column_prefix_sql,
            position_at_end(alter_change_column_prefix_sql)
        ),
        CompletionContext::ColumnTargetClause
    );

    let alter_action_sql = "ALTER TABLE app.users ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_action_sql,
            position_at_end(alter_action_sql)
        ),
        CompletionContext::AlterTableActionClause
    );

    let alter_action_prefix_sql = "ALTER TABLE app.users DR";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_action_prefix_sql,
            position_at_end(alter_action_prefix_sql)
        ),
        CompletionContext::AlterTableActionClause
    );

    let alter_drop_constraint_sql = "ALTER TABLE app.users DROP CONSTRAINT ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_drop_constraint_sql,
            position_at_end(alter_drop_constraint_sql)
        ),
        CompletionContext::ConstraintTargetClause
    );

    let alter_drop_index_sql = "ALTER TABLE app.users DROP INDEX ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_drop_index_sql,
            position_at_end(alter_drop_index_sql)
        ),
        CompletionContext::IndexTargetClause
    );

    let create_table_type_sql = "CREATE TABLE app.users (name ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            create_table_type_sql,
            position_at_end(create_table_type_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let create_table_type_prefix_sql = "CREATE TABLE app.users (name var";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            create_table_type_prefix_sql,
            position_at_end(create_table_type_prefix_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_add_column_type_sql = "ALTER TABLE app.users ADD COLUMN status ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_add_column_type_sql,
            position_at_end(alter_add_column_type_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_add_column_type_prefix_sql = "ALTER TABLE app.users ADD COLUMN status var";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_add_column_type_prefix_sql,
            position_at_end(alter_add_column_type_prefix_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_add_without_column_type_sql = "ALTER TABLE app.users ADD status ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_add_without_column_type_sql,
            position_at_end(alter_add_without_column_type_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_modify_column_type_sql = "ALTER TABLE app.users MODIFY COLUMN status ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_modify_column_type_sql,
            position_at_end(alter_modify_column_type_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_modify_type_prefix_sql = "ALTER TABLE app.users MODIFY status var";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_modify_type_prefix_sql,
            position_at_end(alter_modify_type_prefix_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_change_column_type_sql = "ALTER TABLE app.users CHANGE COLUMN status state ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_change_column_type_sql,
            position_at_end(alter_change_column_type_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_change_type_prefix_sql = "ALTER TABLE app.users CHANGE status state var";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_change_type_prefix_sql,
            position_at_end(alter_change_type_prefix_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_column_type_sql = "ALTER TABLE app.users ALTER COLUMN status TYPE ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_column_type_sql,
            position_at_end(alter_column_type_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_column_type_prefix_sql = "ALTER TABLE app.users ALTER COLUMN status TYPE var";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            alter_column_type_prefix_sql,
            position_at_end(alter_column_type_prefix_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let create_table_constraint_sql = "CREATE TABLE app.users (PRIMARY ";
    assert_ne!(
        parser.analyze_completion_context_fallback(
            create_table_constraint_sql,
            position_at_end(create_table_constraint_sql)
        ),
        CompletionContext::DataTypeClause
    );

    let alter_add_index_sql = "ALTER TABLE app.users ADD INDEX users_name_idx";
    assert_ne!(
        parser.analyze_completion_context_fallback(
            alter_add_index_sql,
            position_at_end(alter_add_index_sql)
        ),
        CompletionContext::DataTypeClause
    );
}
