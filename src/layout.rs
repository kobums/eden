//! 페인 분할 레이아웃: 탭 하나는 페인들의 이진 분할 트리다.

use crate::session::Session;

/// 페인 사이 구분선 두께 (물리 픽셀).
const GAP: f32 = 3.0;

/// 구분선 드래그 판정 여유 (물리 픽셀). GAP이 3px라 그대로는 잡기 어렵다.
pub const DIVIDER_GRAB: f32 = 4.0;

/// 분할 비율의 상하한. 한쪽 페인이 사라질 만큼 끌지 못하게 막는다.
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;

/// 루트에서 특정 Split까지 내려가는 경로 (false = first, true = second).
pub type SplitPath = Vec<bool>;

/// 드래그 가능한 구분선 하나.
#[derive(Clone, Debug)]
pub struct Divider {
    /// 이 구분선을 소유한 Split까지의 경로.
    pub path: SplitPath,
    /// 히트 영역 (GAP보다 넉넉하다).
    pub rect: Rect,
    pub dir: SplitDir,
    /// Split이 차지하는 전체 영역 — 드래그 위치를 비율로 환산할 때 쓴다.
    pub bounds: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    /// 좌우 배치 (세로 구분선)
    Row,
    /// 상하 배치 (가로 구분선)
    Column,
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 한 사각형을 방향과 비율에 따라 둘로 나눈다 (사이에 GAP).
///
/// 배치(`layout`)와 구분선 위치(`dividers`)가 같은 계산을 써야 하므로
/// 한 곳에 둔다 — 어긋나면 구분선이 실제 경계와 다른 자리에 잡힌다.
fn split_rects(rect: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    match dir {
        SplitDir::Row => {
            let w1 = ((rect.w - GAP) * ratio).floor();
            (
                Rect { w: w1, ..rect },
                Rect {
                    x: rect.x + w1 + GAP,
                    w: rect.w - w1 - GAP,
                    ..rect
                },
            )
        }
        SplitDir::Column => {
            let h1 = ((rect.h - GAP) * ratio).floor();
            (
                Rect { h: h1, ..rect },
                Rect {
                    y: rect.y + h1 + GAP,
                    h: rect.h - h1 - GAP,
                    ..rect
                },
            )
        }
    }
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (x, y) = (x as f32, y as f32);
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

pub struct Pane {
    pub id: usize,
    pub session: Session,
    pub title: String,
    /// 보고 있지 않을 때 끝난 명령의 종료 코드 — 탭 바에 점으로 표시되고,
    /// 그 탭을 다시 보면(그려지면) 지워진다. 바깥 Option은 "미확인 결과가
    /// 있는가", 안쪽은 D 마크의 종료 코드(없을 수 있다)다.
    pub unseen_exit: Option<Option<i32>>,
}

/// 분할 트리가 페이로드에서 요구하는 전부 — 식별자 하나.
///
/// 트리를 `Pane`이 아니라 이 트레이트로 일반화한 이유는 테스트 때문이다.
/// `Pane`은 `Session`을 소유하고 `Session`은 mux 데몬을 띄우므로 단위
/// 테스트에서 만들 수 없다. 테스트는 `PaneNode<usize>`를 쓴다.
pub trait PaneId {
    fn pane_id(&self) -> usize;
}

impl PaneId for Pane {
    fn pane_id(&self) -> usize {
        self.id
    }
}

pub enum PaneNode<P = Pane> {
    /// 소유권 이동을 위한 일시적 상태 — 트리 조작 중에만 존재한다.
    Empty,
    Leaf(P),
    Split {
        dir: SplitDir,
        /// `first`가 차지하는 비율 (0.1~0.9). 기본 0.5.
        ratio: f32,
        first: Box<PaneNode<P>>,
        second: Box<PaneNode<P>>,
    },
}

impl<P: PaneId> PaneNode<P> {
    /// 주어진 사각형을 트리 구조대로 나눠 각 페인의 사각형을 계산한다.
    pub fn layout(&self, rect: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            PaneNode::Empty => {}
            PaneNode::Leaf(pane) => out.push((pane.pane_id(), rect)),
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rects(rect, *dir, *ratio);
                first.layout(a, out);
                second.layout(b, out);
            }
        }
    }

    /// 줌을 반영한 배치. 줌 중이면 `focused` 페인 하나가 전체를 차지한다.
    ///
    /// 트리 자체는 건드리지 않으므로 줌을 풀면 원래 구조가 그대로 돌아온다.
    /// `focused`가 트리에 없으면(경합으로 이미 닫힌 경우) 줌을 무시하고
    /// 정상 배치로 떨어진다 — 아무것도 그리지 않는 것보다 낫다.
    pub fn layout_zoomed(&self, rect: Rect, focused: usize, zoomed: bool) -> Vec<(usize, Rect)> {
        if zoomed && self.pane(focused).is_some() {
            return vec![(focused, rect)];
        }
        let mut out = Vec::new();
        self.layout(rect, &mut out);
        out
    }

    /// 드래그 가능한 구분선들을 모은다. 줌 중에는 구분선이 보이지 않으므로
    /// 호출자가 건너뛴다.
    pub fn dividers(&self, rect: Rect) -> Vec<Divider> {
        let mut out = Vec::new();
        self.collect_dividers(rect, &mut Vec::new(), &mut out);
        out
    }

    fn collect_dividers(&self, rect: Rect, path: &mut SplitPath, out: &mut Vec<Divider>) {
        let PaneNode::Split {
            dir,
            ratio,
            first,
            second,
        } = self
        else {
            return;
        };

        // 구분선은 first 페인이 끝나는 자리다. 배치와 같은 계산에서 끌어내므로
        // 둘이 어긋날 수 없다.
        let (first_rect, second_rect) = split_rects(rect, *dir, *ratio);
        let divider = match dir {
            SplitDir::Row => Rect {
                x: first_rect.x + first_rect.w - DIVIDER_GRAB,
                y: rect.y,
                w: GAP + DIVIDER_GRAB * 2.0,
                h: rect.h,
            },
            SplitDir::Column => Rect {
                x: rect.x,
                y: first_rect.y + first_rect.h - DIVIDER_GRAB,
                w: rect.w,
                h: GAP + DIVIDER_GRAB * 2.0,
            },
        };
        out.push(Divider {
            path: path.clone(),
            rect: divider,
            dir: *dir,
            bounds: rect,
        });

        path.push(false);
        first.collect_dividers(first_rect, path, out);
        path.pop();
        path.push(true);
        second.collect_dividers(second_rect, path, out);
        path.pop();
    }

    /// 경로가 가리키는 Split의 비율을 바꾼다. 경로가 틀리면 아무것도 안 한다.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        let mut node = self;
        for &branch in path {
            let PaneNode::Split { first, second, .. } = node else {
                return;
            };
            node = if branch { second } else { first };
        }
        if let PaneNode::Split { ratio: r, .. } = node {
            *r = ratio.clamp(MIN_RATIO, MAX_RATIO);
        }
    }

    pub fn panes(&self) -> Vec<&P> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a P>) {
        match self {
            PaneNode::Empty => {}
            PaneNode::Leaf(pane) => out.push(pane),
            PaneNode::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    pub fn pane(&self, id: usize) -> Option<&P> {
        self.panes().into_iter().find(|p| p.pane_id() == id)
    }

    pub fn pane_mut(&mut self, id: usize) -> Option<&mut P> {
        match self {
            PaneNode::Empty => None,
            PaneNode::Leaf(pane) => (pane.pane_id() == id).then_some(pane),
            PaneNode::Split { first, second, .. } => {
                first.pane_mut(id).or_else(|| second.pane_mut(id))
            }
        }
    }

    pub fn first_id(&self) -> Option<usize> {
        self.panes().first().map(|p| p.pane_id())
    }

    /// `target` 리프를 (기존, 새 페인)의 분할로 교체한다.
    /// 성공하면 None, 실패하면 새 페인을 되돌려준다.
    pub fn split_leaf(&mut self, target: usize, dir: SplitDir, new_pane: P) -> Option<P> {
        match self {
            PaneNode::Leaf(pane) if pane.pane_id() == target => {
                let old = std::mem::replace(self, PaneNode::Empty);
                *self = PaneNode::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(old),
                    second: Box::new(PaneNode::Leaf(new_pane)),
                };
                None
            }
            PaneNode::Split { first, second, .. } => {
                match first.split_leaf(target, dir, new_pane) {
                    None => None,
                    Some(returned) => second.split_leaf(target, dir, returned),
                }
            }
            _ => Some(new_pane),
        }
    }

    /// `target` 리프를 제거하고 형제를 부모 자리로 올린다.
    /// 루트가 단일 리프인 경우는 제거하지 않는다 (탭 닫기는 호출자 책임).
    pub fn remove(&mut self, target: usize) -> bool {
        if let PaneNode::Split { first, second, .. } = self {
            if matches!(&**first, PaneNode::Leaf(p) if p.pane_id() == target) {
                let sibling = std::mem::replace(&mut **second, PaneNode::Empty);
                *self = sibling;
                return true;
            }
            if matches!(&**second, PaneNode::Leaf(p) if p.pane_id() == target) {
                let sibling = std::mem::replace(&mut **first, PaneNode::Empty);
                *self = sibling;
                return true;
            }
            return first.remove(target) || second.remove(target);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pane은 Session(→ mux 데몬)을 소유해 테스트에서 만들 수 없으므로
    // 페이로드로 usize를 쓴다. PaneId 덕에 트리 로직은 동일하다.
    impl PaneId for usize {
        fn pane_id(&self) -> usize {
            *self
        }
    }

    type Node = PaneNode<usize>;

    fn leaf(id: usize) -> Node {
        PaneNode::Leaf(id)
    }

    fn split(dir: SplitDir, first: Node, second: Node) -> Node {
        PaneNode::Split {
            dir,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn laid_out(node: &Node, r: Rect) -> Vec<(usize, Rect)> {
        let mut out = Vec::new();
        node.layout(r, &mut out);
        out
    }

    #[test]
    fn contains_includes_top_left_excludes_bottom_right() {
        let r = rect(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains(10.0, 20.0), "좌상단 모서리는 포함");
        assert!(r.contains(109.9, 69.9));
        assert!(!r.contains(110.0, 40.0), "우측 경계는 배제");
        assert!(!r.contains(50.0, 70.0), "하단 경계는 배제");
        assert!(!r.contains(9.9, 20.0));
        assert!(!r.contains(10.0, 19.9));
    }

    #[test]
    fn center_is_midpoint() {
        assert_eq!(rect(0.0, 0.0, 100.0, 50.0).center(), (50.0, 25.0));
        assert_eq!(rect(10.0, 20.0, 100.0, 50.0).center(), (60.0, 45.0));
    }

    #[test]
    fn single_leaf_fills_the_rect() {
        let out = laid_out(&leaf(7), rect(0.0, 0.0, 800.0, 600.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 7);
        assert_eq!((out[0].1.x, out[0].1.y), (0.0, 0.0));
        assert_eq!((out[0].1.w, out[0].1.h), (800.0, 600.0));
    }

    #[test]
    fn empty_node_yields_nothing() {
        assert!(laid_out(&PaneNode::Empty, rect(0.0, 0.0, 800.0, 600.0)).is_empty());
    }

    #[test]
    fn row_split_halves_width_and_accounts_for_the_gap() {
        let tree = split(SplitDir::Row, leaf(1), leaf(2));
        let out = laid_out(&tree, rect(0.0, 0.0, 801.0, 600.0));

        assert_eq!(out.len(), 2);
        let (first, second) = (out[0].1, out[1].1);

        // 폭 합 + 구분선 = 전체 폭 (반올림 손실 없음)
        assert_eq!(first.w + GAP + second.w, 801.0);
        assert_eq!(second.x, first.x + first.w + GAP);
        // 높이는 나뉘지 않는다
        assert_eq!(first.h, 600.0);
        assert_eq!(second.h, 600.0);
        assert_eq!(first.y, second.y);
    }

    #[test]
    fn column_split_halves_height_and_accounts_for_the_gap() {
        let tree = split(SplitDir::Column, leaf(1), leaf(2));
        let out = laid_out(&tree, rect(0.0, 0.0, 800.0, 601.0));

        let (first, second) = (out[0].1, out[1].1);
        assert_eq!(first.h + GAP + second.h, 601.0);
        assert_eq!(second.y, first.y + first.h + GAP);
        assert_eq!(first.w, 800.0);
        assert_eq!(second.w, 800.0);
        assert_eq!(first.x, second.x);
    }

    #[test]
    fn nested_split_lays_out_three_panes_without_overlap() {
        // 좌: 1, 우: 상 2 / 하 3
        let tree = split(
            SplitDir::Row,
            leaf(1),
            split(SplitDir::Column, leaf(2), leaf(3)),
        );
        let out = laid_out(&tree, rect(0.0, 0.0, 800.0, 600.0));

        assert_eq!(
            out.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let (p1, p2, p3) = (out[0].1, out[1].1, out[2].1);
        assert_eq!(p2.x, p3.x, "우측 두 페인은 같은 열");
        assert_eq!(p2.w, p3.w);
        assert!(p2.y < p3.y, "2가 3 위에");
        assert_eq!(p1.h, 600.0, "좌측 페인은 전체 높이");
        assert!(p1.x + p1.w <= p2.x, "좌우가 겹치지 않음");
    }

    #[test]
    fn ratio_shifts_the_split_boundary() {
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        let full = rect(0.0, 0.0, 803.0, 600.0);

        let even = laid_out(&tree, full);
        assert_eq!(even[0].1.w, even[1].1.w, "0.5면 좌우가 같다");

        tree.set_ratio(&[], 0.25);
        let out = laid_out(&tree, full);
        assert!(out[0].1.w < out[1].1.w, "왼쪽이 좁아진다");
        assert_eq!(out[0].1.w + GAP + out[1].1.w, 803.0, "폭 합은 그대로");
        assert_eq!(out[1].1.x, out[0].1.x + out[0].1.w + GAP);
    }

    #[test]
    fn ratio_is_clamped_so_a_pane_never_vanishes() {
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        let full = rect(0.0, 0.0, 800.0, 600.0);

        tree.set_ratio(&[], -5.0);
        let out = laid_out(&tree, full);
        assert!(out[0].1.w > 0.0, "왼쪽이 사라지면 안 된다");

        tree.set_ratio(&[], 99.0);
        let out = laid_out(&tree, full);
        assert!(out[1].1.w > 0.0, "오른쪽도 마찬가지");
    }

    #[test]
    fn divider_sits_exactly_where_the_first_pane_ends() {
        let tree = split(SplitDir::Row, leaf(1), leaf(2));
        let full = rect(0.0, 0.0, 803.0, 600.0);

        let panes = laid_out(&tree, full);
        let dividers = tree.dividers(full);
        assert_eq!(dividers.len(), 1);

        let d = &dividers[0];
        let first_end = panes[0].1.x + panes[0].1.w;
        // 히트 영역은 경계를 GAP만큼 감싸고 여유(GRAB)를 양쪽에 둔다.
        assert_eq!(d.rect.x, first_end - DIVIDER_GRAB);
        assert_eq!(d.rect.w, GAP + DIVIDER_GRAB * 2.0);
        assert!(
            d.rect.contains(first_end as f64, 300.0),
            "경계를 잡을 수 있다"
        );
        assert_eq!(d.dir, SplitDir::Row);
    }

    #[test]
    fn nested_splits_yield_addressable_dividers() {
        // 좌: 1, 우: 상 2 / 하 3  → 구분선 2개 (루트 세로, 오른쪽 가로)
        let tree = split(
            SplitDir::Row,
            leaf(1),
            split(SplitDir::Column, leaf(2), leaf(3)),
        );
        let full = rect(0.0, 0.0, 800.0, 600.0);
        let dividers = tree.dividers(full);

        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].path, Vec::<bool>::new(), "루트 Split");
        assert_eq!(dividers[0].dir, SplitDir::Row);
        assert_eq!(dividers[1].path, vec![true], "second 쪽 Split");
        assert_eq!(dividers[1].dir, SplitDir::Column);
    }

    #[test]
    fn set_ratio_targets_the_addressed_split_only() {
        let mut tree = split(
            SplitDir::Row,
            leaf(1),
            split(SplitDir::Column, leaf(2), leaf(3)),
        );
        let full = rect(0.0, 0.0, 800.0, 600.0);
        let before = laid_out(&tree, full);

        // 중첩된 Split만 바꾼다 — 루트는 그대로여야 한다.
        tree.set_ratio(&[true], 0.8);
        let after = laid_out(&tree, full);

        assert_eq!(before[0].1.w, after[0].1.w, "루트 비율은 불변");
        assert!(after[1].1.h > before[1].1.h, "2번 페인이 커진다");
    }

    #[test]
    fn set_ratio_ignores_a_path_that_does_not_exist() {
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        let full = rect(0.0, 0.0, 800.0, 600.0);
        let before = laid_out(&tree, full);

        // leaf로 내려가는 경로 — 패닉 없이 무시되어야 한다.
        tree.set_ratio(&[false, true, false], 0.9);
        let after = laid_out(&tree, full);
        assert_eq!(before[0].1.w, after[0].1.w);
    }

    #[test]
    fn a_leaf_has_no_dividers() {
        assert!(leaf(1).dividers(rect(0.0, 0.0, 800.0, 600.0)).is_empty());
    }

    #[test]
    fn zoom_gives_the_focused_pane_the_whole_rect() {
        let tree = split(
            SplitDir::Row,
            leaf(1),
            split(SplitDir::Column, leaf(2), leaf(3)),
        );
        let full = rect(0.0, 0.0, 800.0, 600.0);

        let out = tree.layout_zoomed(full, 2, true);
        assert_eq!(out.len(), 1, "줌 중에는 한 페인만 배치된다");
        assert_eq!(out[0].0, 2, "포커스된 페인이어야 한다");
        assert_eq!((out[0].1.w, out[0].1.h), (800.0, 600.0), "전체를 차지");
        assert_eq!((out[0].1.x, out[0].1.y), (0.0, 0.0));
    }

    #[test]
    fn unzoom_restores_the_original_split_exactly() {
        let tree = split(
            SplitDir::Row,
            leaf(1),
            split(SplitDir::Column, leaf(2), leaf(3)),
        );
        let full = rect(0.0, 0.0, 800.0, 600.0);

        let before = tree.layout_zoomed(full, 2, false);
        let _zoomed = tree.layout_zoomed(full, 2, true);
        let after = tree.layout_zoomed(full, 2, false);

        // 트리를 건드리지 않으므로 줌 전후 배치가 완전히 같아야 한다.
        assert_eq!(before.len(), 3);
        assert_eq!(after.len(), 3);
        for (b, a) in before.iter().zip(after.iter()) {
            assert_eq!(b.0, a.0);
            assert_eq!((b.1.x, b.1.y, b.1.w, b.1.h), (a.1.x, a.1.y, a.1.w, a.1.h));
        }
    }

    #[test]
    fn zoom_falls_back_when_the_focused_pane_is_gone() {
        // focused가 트리에 없으면(이미 닫힌 페인) 줌을 무시한다 —
        // 그러지 않으면 존재하지 않는 페인 하나만 배치돼 화면이 빈다.
        let tree = split(SplitDir::Row, leaf(1), leaf(2));
        let out = tree.layout_zoomed(rect(0.0, 0.0, 800.0, 600.0), 99, true);
        assert_eq!(out.len(), 2, "정상 배치로 떨어진다");
    }

    #[test]
    fn zoom_on_a_single_leaf_is_harmless() {
        let tree = leaf(1);
        let full = rect(0.0, 0.0, 800.0, 600.0);
        let out = tree.layout_zoomed(full, 1, true);
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].1.w, out[0].1.h),
            (800.0, 600.0),
            "줌 안 한 것과 같다"
        );
    }

    #[test]
    fn panes_and_first_id_walk_in_order() {
        let tree = split(
            SplitDir::Row,
            leaf(5),
            split(SplitDir::Column, leaf(6), leaf(7)),
        );
        assert_eq!(tree.panes(), vec![&5, &6, &7]);
        assert_eq!(tree.first_id(), Some(5));
        assert_eq!(PaneNode::<usize>::Empty.first_id(), None);
    }

    #[test]
    fn pane_lookup_finds_present_and_rejects_absent() {
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        assert_eq!(tree.pane(2), Some(&2));
        assert_eq!(tree.pane(9), None);
        assert_eq!(tree.pane_mut(1), Some(&mut 1));
        assert_eq!(tree.pane_mut(9), None);
    }

    #[test]
    fn split_leaf_replaces_the_target_with_a_split() {
        let mut tree = leaf(1);
        assert!(tree.split_leaf(1, SplitDir::Row, 2).is_none());
        assert_eq!(tree.panes(), vec![&1, &2]);
        assert!(matches!(
            tree,
            PaneNode::Split {
                dir: SplitDir::Row,
                ..
            }
        ));
    }

    #[test]
    fn split_leaf_recurses_into_the_second_subtree() {
        // 타깃이 second 아래에만 있을 때도 찾아야 한다 (재귀 경로).
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        assert!(tree.split_leaf(2, SplitDir::Column, 3).is_none());
        assert_eq!(tree.panes(), vec![&1, &2, &3]);
    }

    #[test]
    fn split_leaf_returns_the_pane_when_target_is_absent() {
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        assert_eq!(tree.split_leaf(99, SplitDir::Row, 3), Some(3));
        assert_eq!(tree.panes(), vec![&1, &2], "트리는 그대로");
    }

    #[test]
    fn remove_promotes_the_sibling_from_either_side() {
        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        assert!(tree.remove(1));
        assert_eq!(tree.panes(), vec![&2]);
        assert!(matches!(tree, PaneNode::Leaf(2)), "형제가 부모 자리로");

        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        assert!(tree.remove(2));
        assert!(matches!(tree, PaneNode::Leaf(1)));
    }

    #[test]
    fn remove_recurses_and_keeps_the_rest_of_the_tree() {
        let mut tree = split(
            SplitDir::Row,
            leaf(1),
            split(SplitDir::Column, leaf(2), leaf(3)),
        );
        assert!(tree.remove(3));
        assert_eq!(tree.panes(), vec![&1, &2]);
    }

    #[test]
    fn remove_refuses_to_empty_a_root_leaf() {
        // 마지막 페인 제거는 탭 닫기이므로 호출자 책임 — 트리는 거부한다.
        let mut tree = leaf(1);
        assert!(!tree.remove(1));
        assert_eq!(tree.panes(), vec![&1]);

        let mut tree = split(SplitDir::Row, leaf(1), leaf(2));
        assert!(!tree.remove(99), "없는 타깃");
    }
}
