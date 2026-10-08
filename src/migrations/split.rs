//! Splitting migration scripts into individual statements.
//!
//! Statements are executed exactly as written: the splitter only finds the top-level `;`
//! terminators. It skips over string literals, quoted identifiers, comments and (on
//! PostgreSQL) dollar-quoted bodies, so a `;` inside any of those never ends a statement.
//! Statements that contain nothing but whitespace and comments are dropped.

/// SQL dialect rules that affect where a statement can end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// PostgreSQL: `$tag$ ... $tag$` bodies; `\` is not an escape in standard strings.
    Postgres,
    /// MySQL: backtick identifiers, `#` comments and `\` escapes inside strings.
    MySql,
}

/// Split `sql` into statements, each returned without its terminating `;`.
pub(crate) fn split_statements(sql: &str, dialect: Dialect) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut statements = Vec::new();
    let mut start = 0;
    let mut has_code = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' | '"' => {
                i = skip_quoted(&chars, i, c, dialect == Dialect::MySql);
                has_code = true;
                continue;
            }
            '`' if dialect == Dialect::MySql => {
                i = skip_quoted(&chars, i, '`', false);
                has_code = true;
                continue;
            }
            '-' if next == Some('-') => {
                i = skip_line(&chars, i);
                continue;
            }
            '#' if dialect == Dialect::MySql => {
                i = skip_line(&chars, i);
                continue;
            }
            '/' if next == Some('*') => {
                i = skip_block_comment(&chars, i);
                continue;
            }
            '$' if dialect == Dialect::Postgres => {
                if let Some(end) = skip_dollar_quoted(&chars, i) {
                    i = end;
                    has_code = true;
                    continue;
                }
                has_code = true;
            }
            ';' => {
                if has_code {
                    statements.push(
                        chars[start..i]
                            .iter()
                            .collect::<String>()
                            .trim()
                            .to_string(),
                    );
                }
                start = i + 1;
                has_code = false;
            }
            c if !c.is_whitespace() => has_code = true,
            _ => {}
        }
        i += 1;
    }

    if has_code {
        statements.push(chars[start..].iter().collect::<String>().trim().to_string());
    }
    statements
}

/// Skip a quoted run starting at `open` (the opening quote). A doubled quote is an
/// escaped quote; with `backslash_escapes`, `\` escapes the next character.
/// Returns the index just past the closing quote (or the end of input).
fn skip_quoted(chars: &[char], open: usize, quote: char, backslash_escapes: bool) -> usize {
    let mut i = open + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if backslash_escapes => i += 2,
            c if c == quote => {
                if chars.get(i + 1) == Some(&quote) {
                    i += 2;
                } else {
                    return i + 1;
                }
            }
            _ => i += 1,
        }
    }
    chars.len()
}

/// Skip to the end of the line (the newline is not consumed).
fn skip_line(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && chars[i] != '\n' {
        i += 1;
    }
    i
}

/// Skip a `/* ... */` comment; PostgreSQL allows nesting, and nesting is harmless to
/// honour for MySQL migrations, which never nest.
fn skip_block_comment(chars: &[char], start: usize) -> usize {
    let mut depth = 0;
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            depth += 1;
            i += 2;
        } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    chars.len()
}

/// If a dollar-quote opener (`$$` or `$tag$`) starts at `start`, return the index just
/// past its matching closer (or the end of input). Otherwise (e.g. a `$1` parameter)
/// return `None`.
fn skip_dollar_quoted(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
        i += 1;
    }
    if chars.get(i) != Some(&'$') {
        return None;
    }
    let tag = &chars[start..=i];
    // A tag can't start with a digit: `$1$` is not a dollar quote.
    if tag.len() > 2 && tag[1].is_ascii_digit() {
        return None;
    }
    let mut j = i + 1;
    while j + tag.len() <= chars.len() {
        if chars[j..j + tag.len()] == *tag {
            return Some(j + tag.len());
        }
        j += 1;
    }
    Some(chars.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg(sql: &str) -> Vec<String> {
        split_statements(sql, Dialect::Postgres)
    }

    fn my(sql: &str) -> Vec<String> {
        split_statements(sql, Dialect::MySql)
    }

    #[test]
    fn splits_on_top_level_semicolons_and_keeps_text_verbatim() {
        assert_eq!(
            pg("CREATE TABLE a (x INT);\n\nCREATE INDEX i ON a (x) ;"),
            vec!["CREATE TABLE a (x INT)", "CREATE INDEX i ON a (x)"]
        );
    }

    #[test]
    fn final_statement_without_semicolon_is_kept() {
        assert_eq!(pg("SELECT 1; SELECT 2"), vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn comment_only_and_empty_statements_are_dropped() {
        assert_eq!(
            pg("-- header; with a semicolon\n;\n/* block; */ ;\nSELECT 1;\n-- trailing"),
            vec!["SELECT 1"]
        );
    }

    #[test]
    fn semicolons_in_strings_and_identifiers_do_not_split() {
        assert_eq!(
            pg(r#"INSERT INTO t VALUES ('a;b', 'it''s; fine'); SELECT "odd;name" FROM t;"#),
            vec![
                "INSERT INTO t VALUES ('a;b', 'it''s; fine')",
                r#"SELECT "odd;name" FROM t"#
            ]
        );
        assert_eq!(
            my(r"INSERT INTO t VALUES ('a\';b'); SELECT `x;y` FROM t;"),
            vec![r"INSERT INTO t VALUES ('a\';b')", "SELECT `x;y` FROM t"]
        );
    }

    #[test]
    fn comments_can_contain_quotes_and_semicolons() {
        assert_eq!(
            pg("-- don't split; here\nSELECT 1; /* it's; nested /* ; */ */ SELECT 2;"),
            vec![
                "-- don't split; here\nSELECT 1",
                "/* it's; nested /* ; */ */ SELECT 2"
            ]
        );
        assert_eq!(
            my("# it's; a comment\nSELECT 1;"),
            vec!["# it's; a comment\nSELECT 1"]
        );
    }

    #[test]
    fn postgres_dollar_quoted_bodies_are_one_statement() {
        let sql = "CREATE FUNCTION f() RETURNS trigger AS $$\nBEGIN\n  NEW.x := 1;\n  RETURN NEW;\nEND;\n$$ LANGUAGE plpgsql;\n\
                   DO $body$ BEGIN PERFORM 1; END $body$;\n\
                   CREATE TRIGGER t BEFORE UPDATE ON a FOR EACH ROW EXECUTE FUNCTION f();";
        let statements = pg(sql);
        assert_eq!(statements.len(), 3, "{statements:#?}");
        assert!(statements[0].ends_with("$$ LANGUAGE plpgsql"));
        assert_eq!(statements[1], "DO $body$ BEGIN PERFORM 1; END $body$");
        // Executed verbatim: the empty argument list is preserved.
        assert!(statements[2].ends_with("EXECUTE FUNCTION f()"));
    }

    #[test]
    fn positional_parameters_are_not_dollar_quotes() {
        assert_eq!(pg("SELECT $1; SELECT $2;"), vec!["SELECT $1", "SELECT $2"]);
    }

    #[test]
    fn mysql_does_not_treat_dollar_signs_specially() {
        assert_eq!(
            my("SELECT '$$'; SELECT 2;"),
            vec!["SELECT '$$'", "SELECT 2"]
        );
    }

    #[test]
    fn mysql_prepared_ddl_pattern_splits_per_statement() {
        let sql = "SET @sql = IF((SELECT COUNT(*) FROM information_schema.columns \
                   WHERE column_name = 'x') = 0, 'ALTER TABLE t ADD COLUMN x INT', 'SELECT 1');\n\
                   PREPARE stmt FROM @sql;\nEXECUTE stmt;\nDEALLOCATE PREPARE stmt;";
        assert_eq!(my(sql).len(), 4);
    }

    #[test]
    fn every_bundled_migration_splits_into_non_empty_statements() {
        for (name, sql, dialect) in [
            (
                "012 pg",
                include_str!("012_optimize_dependencies.postgres.sql"),
                Dialect::Postgres,
            ),
            (
                "014 pg",
                include_str!("014_add_queue_pause.postgres.sql"),
                Dialect::Postgres,
            ),
            (
                "010 mysql",
                include_str!("010_add_archival.mysql.sql"),
                Dialect::MySql,
            ),
            (
                "011 mysql",
                include_str!("011_add_encryption.mysql.sql"),
                Dialect::MySql,
            ),
        ] {
            let statements = split_statements(sql, dialect);
            assert!(!statements.is_empty(), "{name}");
            for statement in &statements {
                assert!(!statement.trim().is_empty(), "{name}: empty statement");
                assert!(!statement.ends_with(';'), "{name}: {statement}");
            }
        }
    }
}
