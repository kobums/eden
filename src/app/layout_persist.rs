//! 분할 레이아웃 저장/복원 — `~/.cache/eden/layout.json`.
//!
//! 데몬은 건드리지 않는다. 레이아웃은 GUI(클라이언트)의 관심사고, 데몬이
//! 죽어 세션이 사라지면 파일은 가지치기로 자연히 무시되므로 클라이언트
//! 파일 하나로 충분하다.
//!
//! layout.rs에 serde derive를 붙이지 않는 이유: `Pane`이 `Session`(소켓·
//! 스레드)을 소유해 Serialize가 불가능하다. 트리 구조만 옮기면 되므로
//! `serde_json::Value` 수동 변환이 오히려 짧다. leaf에는 mux 세션 ID(u64)를
//! 넣는다 — 페인 ID는 실행마다 달라지는 런타임 값이라 쓸 수 없다.
//! `zoomed`는 일시 상태라 저장하지 않는다.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::layout::{Pane, PaneId, PaneNode, SplitDir};

/// 스키마 버전. 파일의 버전이 다르면(미래 버전 포함) 통째로 무시하고
/// 기본 복원(세션당 탭 1개)으로 폴백한다 — 절반만 이해하고 복원하는 것보다
/// 안전하다.
const VERSION: u64 = 1;

/// 탭 하나의 복원 계획. leaf 페이로드는 mux 세션 ID다.
pub struct TabLayout {
    pub root: PaneNode<u64>,
    /// 포커스된 leaf의 세션 ID.
    pub focused: u64,
}

// 세션 ID 트리에도 `first_id`·`pane` 같은 제네릭 메서드를 쓰기 위한 구현.
// `pane_id`가 usize라 u64→usize 캐스트가 끼지만, 64비트 대상에서 손실이
// 없고 세션 ID는 데몬이 1부터 순차 발급하므로 실질적으로도 안전하다.
impl PaneId for u64 {
    fn pane_id(&self) -> usize {
        *self as usize
    }
}

/// 저장 파일 경로 (mux 소켓과 같은 캐시 디렉터리).
fn layout_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".cache/eden/layout.json")
}

/// 레이아웃 JSON을 파일에 쓴다. 실패는 조용히 무시한다 — 저장이 안 되면
/// 다음 실행이 기본 복원으로 폴백할 뿐, 지금 세션에는 영향이 없다.
pub fn save(v: &Value) {
    let path = layout_path();
    let Some(parent) = path.parent() else { return };
    let _ = std::fs::create_dir_all(parent);
    // 임시 파일 + rename — 쓰는 도중 크래시해도 반쪽짜리 JSON이 남지 않는다.
    // 깨진 파일은 어차피 폴백되지만, "크래시에서도 마지막 구조가 남는다"가
    // 저장 시점을 즉시로 정한 이유이므로 직전 파일을 지키는 쪽이 맞다.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 파일을 읽어 파싱한다. 없거나 깨졌으면 None → 호출자가 기본 복원으로 폴백.
pub fn load() -> Option<Value> {
    let text = std::fs::read_to_string(layout_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// 런타임 트리에서 저장용 세션 ID 트리를 뽑는다.
///
/// `focused`는 페인 ID(런타임 값)로 들어오므로 세션 ID로 바꿔 넣는다.
/// 포커스 페인을 트리에서 못 찾으면(경합) 첫 leaf로 폴백한다.
pub fn snapshot(root: &PaneNode<Pane>, focused: usize) -> Option<TabLayout> {
    let ids = node_to_ids(root)?;
    let focused = root
        .pane(focused)
        .map(|p| p.session.id())
        .or_else(|| ids.first_id().map(|i| i as u64))?;
    Some(TabLayout { root: ids, focused })
}

fn node_to_ids(node: &PaneNode<Pane>) -> Option<PaneNode<u64>> {
    match node {
        // Empty는 트리 조작 중에만 있어야 하지만, 저장이 그 순간과 겹쳐도
        // 파일이 깨지지 않게 `remove`와 같은 규칙으로 접는다.
        PaneNode::Empty => None,
        PaneNode::Leaf(p) => Some(PaneNode::Leaf(p.session.id())),
        PaneNode::Split {
            dir,
            ratio,
            first,
            second,
        } => join_split(*dir, *ratio, node_to_ids(first), node_to_ids(second)),
    }
}

/// 자식 둘 중 하나가 비면 남은 자식으로 접는다 — `PaneNode::remove`가
/// 형제를 부모 자리로 올리는 것과 같은 규칙이다.
fn join_split(
    dir: SplitDir,
    ratio: f32,
    first: Option<PaneNode<u64>>,
    second: Option<PaneNode<u64>>,
) -> Option<PaneNode<u64>> {
    match (first, second) {
        (Some(a), Some(b)) => Some(PaneNode::Split {
            dir,
            ratio,
            first: Box::new(a),
            second: Box::new(b),
        }),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// 현재 탭 구조 전체를 JSON으로 만든다.
pub fn to_json(tabs: &[TabLayout], active: usize, boot: Option<u64>) -> Value {
    let tabs: Vec<Value> = tabs
        .iter()
        .map(|t| {
            json!({
                "focused": t.focused,
                "root": node_to_json(&t.root),
            })
        })
        .collect();
    let mut v = json!({
        "version": VERSION,
        "active": active,
        "tabs": tabs,
    });
    // 구버전 데몬(boot id 미지원)에 붙어 있으면 필드 자체를 생략한다 —
    // 0 같은 가짜 값을 넣으면 나중에 진짜 boot id와 무조건 불일치한다.
    if let Some(boot) = boot {
        v["boot"] = json!(boot);
    }
    v
}

fn node_to_json(node: &PaneNode<u64>) -> Value {
    match node {
        PaneNode::Empty => Value::Null, // snapshot이 걸러내므로 도달하지 않는다
        PaneNode::Leaf(id) => json!({ "leaf": id }),
        PaneNode::Split {
            dir,
            ratio,
            first,
            second,
        } => json!({
            "dir": match dir { SplitDir::Row => "row", SplitDir::Column => "column" },
            "ratio": ratio,
            "first": node_to_json(first),
            "second": node_to_json(second),
        }),
    }
}

/// JSON을 복원 계획으로 되돌린다. 반환은 (탭 계획들, 활성 탭 인덱스).
///
/// None이면 호출자가 기본 복원(세션당 탭 1개)으로 폴백한다. 어떤 입력에도
/// 패닉하지 않는다 — 파일은 사용자가 임의로 편집할 수 있는 외부 입력이다.
///
/// 가지치기 규칙 (복원 시 `alive`와 대조):
/// - 죽은 leaf 제거, 자식 하나 남은 Split은 접는다 (`remove`와 동일 규칙)
/// - 모든 leaf가 죽은 탭은 버린다
/// - focused가 죽었으면 그 탭의 첫 leaf로 폴백
/// - 파일에 없는 살아있는 세션은 단독 탭으로 뒤에 붙인다
pub fn from_json(
    v: &Value,
    alive: &HashSet<u64>,
    boot: Option<u64>,
) -> Option<(Vec<TabLayout>, usize)> {
    if v.get("version")?.as_u64()? != VERSION {
        return None;
    }
    // boot id 대조: 데몬이 재시작하면 세션 ID가 1부터 재발급되므로, 죽은
    // 데몬 시절의 파일이 새 데몬의 엉뚱한 세션과 우연히 매칭될 수 있다.
    // 양쪽 다 값이 있고 다를 때만 버린다 — 한쪽이라도 없으면(구버전 데몬·
    // 구버전 파일) 대조를 포기하고 진행하는 것이 하위호환이다.
    if let (Some(file_boot), Some(daemon_boot)) = (v.get("boot").and_then(Value::as_u64), boot)
        && file_boot != daemon_boot
    {
        return None;
    }

    let stored_tabs = v.get("tabs")?.as_array()?;
    let stored_active = v.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;

    let mut out: Vec<TabLayout> = Vec::new();
    let mut active = 0usize;
    let mut used: HashSet<u64> = HashSet::new();
    for (i, t) in stored_tabs.iter().enumerate() {
        // 탭 하나가 깨졌다고 전체를 버리지 않는다 — 살릴 수 있는 탭은 살린다.
        let Some(root) = t.get("root").and_then(node_from_json) else {
            continue;
        };
        let Some(root) = prune(root, alive) else {
            continue; // 모든 leaf 사망 → 탭 버림
        };
        let Some(focused) = t
            .get("focused")
            .and_then(Value::as_u64)
            // 가지치기 후 트리에 남아 있는 leaf만 유효하다.
            .filter(|id| root.pane(*id as usize).is_some())
            .or_else(|| root.first_id().map(|i| i as u64))
        else {
            continue; // prune이 leaf 없는 트리를 돌려주지 않으므로 사실상 도달 불가
        };
        if i == stored_active {
            active = out.len();
        }
        for id in root.panes() {
            used.insert(*id);
        }
        out.push(TabLayout { root, focused });
    }

    // 파일에 없는 살아있는 세션(파일 유실·구버전에서 만든 세션)은 기존
    // 동작대로 단독 탭으로 붙인다. HashSet 순회는 비결정적이므로 ID 순으로
    // 정렬한다 — 데몬이 순차 발급하니 곧 생성 순서다.
    let mut fresh: Vec<u64> = alive
        .iter()
        .copied()
        .filter(|id| !used.contains(id))
        .collect();
    fresh.sort_unstable();
    for id in fresh {
        out.push(TabLayout {
            root: PaneNode::Leaf(id),
            focused: id,
        });
    }

    if out.is_empty() {
        return None;
    }
    if active >= out.len() {
        active = 0;
    }
    Some((out, active))
}

fn node_from_json(v: &Value) -> Option<PaneNode<u64>> {
    if let Some(id) = v.get("leaf").and_then(Value::as_u64) {
        return Some(PaneNode::Leaf(id));
    }
    let dir = match v.get("dir")?.as_str()? {
        "row" => SplitDir::Row,
        "column" => SplitDir::Column,
        _ => return None,
    };
    // ratio가 빠졌거나 이상하면 반반 — set_ratio가 어차피 0.1~0.9로 죈다.
    let ratio = v.get("ratio").and_then(Value::as_f64).unwrap_or(0.5) as f32;
    let first = node_from_json(v.get("first")?)?;
    let second = node_from_json(v.get("second")?)?;
    Some(PaneNode::Split {
        dir,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    })
}

/// 죽은 leaf를 제거하고 자식 하나 남은 Split을 접는다.
fn prune(node: PaneNode<u64>, alive: &HashSet<u64>) -> Option<PaneNode<u64>> {
    match node {
        PaneNode::Empty => None,
        PaneNode::Leaf(id) => alive.contains(&id).then_some(PaneNode::Leaf(id)),
        PaneNode::Split {
            dir,
            ratio,
            first,
            second,
        } => join_split(dir, ratio, prune(*first, alive), prune(*second, alive)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u64) -> PaneNode<u64> {
        PaneNode::Leaf(id)
    }

    fn split(
        dir: SplitDir,
        ratio: f32,
        first: PaneNode<u64>,
        second: PaneNode<u64>,
    ) -> PaneNode<u64> {
        PaneNode::Split {
            dir,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn alive(ids: &[u64]) -> HashSet<u64> {
        ids.iter().copied().collect()
    }

    /// 트리 비교는 JSON 표현으로 한다 — PaneNode에 PartialEq를 붙이는 것보다
    /// "저장되는 형태 그대로"를 비교하는 쪽이 이 모듈의 관심사에 맞다.
    fn tabs_json(tabs: &[TabLayout]) -> Vec<Value> {
        tabs.iter()
            .map(|t| json!({ "focused": t.focused, "root": node_to_json(&t.root) }))
            .collect()
    }

    #[test]
    fn round_trip_preserves_structure_ratio_focus_and_active() {
        let tabs = vec![
            TabLayout {
                root: split(
                    SplitDir::Row,
                    0.6,
                    leaf(3),
                    split(SplitDir::Column, 0.25, leaf(5), leaf(7)),
                ),
                focused: 5,
            },
            TabLayout {
                root: leaf(9),
                focused: 9,
            },
        ];
        let v = to_json(&tabs, 1, Some(42));

        let (restored, active) = from_json(&v, &alive(&[3, 5, 7, 9]), Some(42)).unwrap();
        assert_eq!(active, 1);
        assert_eq!(
            tabs_json(&restored),
            tabs_json(&tabs),
            "구조·비율·포커스 보존"
        );
    }

    #[test]
    fn dead_leaf_folds_the_split_and_keeps_sibling_ratio() {
        // 3-leaf: row(0.6, 1, column(0.25, 2, 3)) 에서 3이 죽으면
        // column Split이 2로 접히고 루트 row의 비율은 그대로여야 한다.
        let tabs = vec![TabLayout {
            root: split(
                SplitDir::Row,
                0.6,
                leaf(1),
                split(SplitDir::Column, 0.25, leaf(2), leaf(3)),
            ),
            focused: 1,
        }];
        let v = to_json(&tabs, 0, None);

        let (restored, _) = from_json(&v, &alive(&[1, 2]), None).unwrap();
        assert_eq!(restored.len(), 1);
        let expected = split(SplitDir::Row, 0.6, leaf(1), leaf(2));
        assert_eq!(node_to_json(&restored[0].root), node_to_json(&expected));
    }

    #[test]
    fn tab_with_all_leaves_dead_is_dropped_and_active_shifts() {
        let tabs = vec![
            TabLayout {
                root: split(SplitDir::Row, 0.5, leaf(1), leaf(2)),
                focused: 1,
            },
            TabLayout {
                root: leaf(3),
                focused: 3,
            },
        ];
        // 활성 탭(1번)만 살아남는다 → 인덱스가 0으로 당겨져야 한다.
        let v = to_json(&tabs, 1, None);
        let (restored, active) = from_json(&v, &alive(&[3]), None).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(node_to_json(&restored[0].root), node_to_json(&leaf(3)));
        assert_eq!(active, 0, "앞 탭이 사라졌으니 활성 인덱스가 당겨진다");
    }

    #[test]
    fn active_falls_back_to_zero_when_the_active_tab_dies() {
        let tabs = vec![
            TabLayout {
                root: leaf(1),
                focused: 1,
            },
            TabLayout {
                root: leaf(2),
                focused: 2,
            },
        ];
        let v = to_json(&tabs, 1, None);
        let (restored, active) = from_json(&v, &alive(&[1]), None).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(active, 0);
    }

    #[test]
    fn dead_focused_falls_back_to_the_first_leaf() {
        let tabs = vec![TabLayout {
            root: split(SplitDir::Row, 0.5, leaf(1), leaf(2)),
            focused: 2,
        }];
        let v = to_json(&tabs, 0, None);
        let (restored, _) = from_json(&v, &alive(&[1]), None).unwrap();
        assert_eq!(restored[0].focused, 1, "first_id 폴백");
    }

    #[test]
    fn alive_sessions_missing_from_the_file_get_their_own_tabs() {
        let tabs = vec![TabLayout {
            root: leaf(1),
            focused: 1,
        }];
        let v = to_json(&tabs, 0, None);
        // 5·3은 파일에 없다 → ID 순서로 단독 탭이 뒤에 붙는다.
        let (restored, _) = from_json(&v, &alive(&[1, 5, 3]), None).unwrap();
        assert_eq!(restored.len(), 3);
        assert_eq!(node_to_json(&restored[1].root), node_to_json(&leaf(3)));
        assert_eq!(restored[1].focused, 3);
        assert_eq!(node_to_json(&restored[2].root), node_to_json(&leaf(5)));
    }

    #[test]
    fn future_version_falls_back() {
        let v =
            json!({ "version": 2, "active": 0, "tabs": [{ "focused": 1, "root": { "leaf": 1 } }] });
        assert!(from_json(&v, &alive(&[1]), None).is_none());
    }

    #[test]
    fn malformed_json_shapes_fall_back_without_panicking() {
        for v in [
            serde_json::from_str::<Value>("null").unwrap(),
            json!([1, 2, 3]),
            json!({ "version": 1 }), // tabs 없음
            json!({ "version": 1, "tabs": "oops" }),
            json!({ "tabs": [] }), // version 없음
        ] {
            assert!(from_json(&v, &alive(&[1]), None).is_none());
        }
        // 파일 내용이 아예 JSON이 아닌 경우는 load()의 파싱 단계에서 걸러진다.
        assert!(serde_json::from_str::<Value>("{깨진 json").is_err());
    }

    #[test]
    fn a_broken_tab_is_skipped_but_the_rest_survive() {
        let v = json!({
            "version": 1,
            "active": 1,
            "tabs": [
                { "focused": 1 },                          // root 없음 → 스킵
                { "focused": 2, "root": { "leaf": 2 } },
            ],
        });
        let (restored, active) = from_json(&v, &alive(&[1, 2]), None).unwrap();
        // 깨진 탭의 세션 1은 "파일에 없는 세션" 취급으로 단독 탭이 된다.
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].focused, 2);
        assert_eq!(active, 0, "저장된 active(1)가 살아남아 0으로 당겨진다");
        assert_eq!(restored[1].focused, 1);
    }

    #[test]
    fn boot_id_mismatch_discards_the_file() {
        let tabs = vec![TabLayout {
            root: leaf(1),
            focused: 1,
        }];
        let v = to_json(&tabs, 0, Some(100));
        // 데몬이 재시작해 boot id가 달라졌다 — 세션 1이 "존재"해도 다른 세션이다.
        assert!(from_json(&v, &alive(&[1]), Some(200)).is_none());
        // 같은 세대면 통과.
        assert!(from_json(&v, &alive(&[1]), Some(100)).is_some());
    }

    #[test]
    fn missing_boot_id_on_either_side_skips_the_check() {
        let tabs = vec![TabLayout {
            root: leaf(1),
            focused: 1,
        }];
        // 구버전 데몬(boot 없음)에서 저장한 파일 + 신버전 데몬 조회.
        let old_file = to_json(&tabs, 0, None);
        assert!(from_json(&old_file, &alive(&[1]), Some(200)).is_some());
        // 신버전 파일 + 구버전 데몬(응답에 boot 없음).
        let new_file = to_json(&tabs, 0, Some(100));
        assert!(from_json(&new_file, &alive(&[1]), None).is_some());
    }

    #[test]
    fn snapshot_like_fold_handles_empty_children() {
        // join_split이 Empty(None) 자식을 remove와 같은 규칙으로 접는지.
        assert!(join_split(SplitDir::Row, 0.5, None, None).is_none());
        let folded = join_split(SplitDir::Row, 0.5, Some(leaf(1)), None).unwrap();
        assert_eq!(node_to_json(&folded), node_to_json(&leaf(1)));
    }
}
