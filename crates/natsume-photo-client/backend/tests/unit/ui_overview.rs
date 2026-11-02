use super::*;

fn team() -> PhotoTeam {
    PhotoTeam {
        account_id: "account-id".into(),
        account: "team001".into(),
        organization_id: 1,
        school_name_zh: "西北工业大学".into(),
        school_name_en: "Northwestern Polytechnical University".into(),
        team_name_zh: "测试队伍".into(),
        team_name_en: "Example Team".into(),
    }
}

#[test]
fn completion_and_school_filters_are_combined() {
    let team = team();
    for completed in [false, true] {
        assert!(overview_matches(&team, completed, 0, ""));
        assert_eq!(overview_matches(&team, completed, 1, "工业"), completed);
        assert_eq!(
            overview_matches(&team, completed, 2, "polytechnical"),
            !completed
        );
        assert!(!overview_matches(&team, completed, 0, "另一学校"));
    }
}

#[test]
fn overview_search_is_school_only_and_english_is_case_insensitive() {
    let team = team();
    assert!(overview_matches(&team, true, 1, "northwestern"));
    assert!(!overview_matches(&team, true, 0, "team001"));
    assert!(!overview_matches(&team, true, 0, "测试队伍"));
    let row = team_row(&team, true);
    assert!(row.r#final);
    assert_eq!(row.school.as_str(), "西北工业大学");
}
