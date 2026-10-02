use quick_panel::{model::*, search};
use rusqlite::Connection;

fn item(title: &str, alias: &str, text: &str) -> StoredItem {
    StoredItem {
        summary: ItemSummary { id: title.into(), kind: ItemKind::Document,
            title: title.into(), subtitle: "C:/工作/接口说明.docx".into(),
            alias: alias.into(), tags: vec![], pinned: false,
            created_at_ms: 1, last_used_ms: 1, use_count: 0 },
        text: text.into(), representations: vec![],
        target: Some("C:/工作/接口说明.docx".into()), open_with: None,
    }
}

#[test]
fn chinese_short_queries_and_pinyin_retrieve_real_fts_rows() {
    let document = item("周报模板", "weekly", "这是今天的接口说明");
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE searchable USING fts5(content)").unwrap();
    db.execute("INSERT INTO searchable(content) VALUES(?1)",
        [search::index_text(&document)]).unwrap();
    for query in ["报", "周报", "接口", "zhoubao", "zbmb"] {
        let expression = search::query_expression(query).unwrap();
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM searchable WHERE searchable MATCH ?1",
            [expression], |row| row.get(0)).unwrap();
        assert_eq!(count, 1, "missing FTS candidate for {query}");
        assert!(search::matches(&document, query), "missing final match for {query}");
    }
}

#[test]
fn literal_user_query_cannot_inject_fts_operators() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE searchable USING fts5(content);
        INSERT INTO searchable(content) VALUES('alpha beta')").unwrap();
    for query in ["alpha\" OR beta", "(alpha):beta", "beta*", "alpha - beta"] {
        let expression = search::query_expression(query).unwrap();
        let _: i64 = db.query_row(
            "SELECT COUNT(*) FROM searchable WHERE searchable MATCH ?1",
            [expression], |row| row.get(0)).unwrap();
    }
}

#[test]
fn exact_alias_beats_pinned_but_less_relevant_item() {
    let exact = item("周报模板", "zb", "周报");
    let mut other = item("zb archive", "", "archive");
    other.summary.pinned = true;
    other.summary.use_count = 100_000;
    assert!(search::rank(&exact.summary, "zb") > search::rank(&other.summary, "zb"));
}

#[test]
fn unrelated_query_is_rejected_and_paths_accept_windows_separators() {
    let document = item("接口说明", "api", "方法参数和返回值");
    assert!(!search::matches(&document, "猫粮"));
    assert!(search::matches(&document, "C:\\工作\\接口"));
    assert!(search::query_expression("   ").is_none());
}
