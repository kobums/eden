//! 페인 분할 레이아웃: 탭 하나는 페인들의 이진 분할 트리다.

use crate::session::Session;

/// 페인 사이 구분선 두께 (물리 픽셀).
const GAP: f32 = 3.0;

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
            PaneNode::Split { dir, first, second } => match dir {
                SplitDir::Row => {
                    let w1 = ((rect.w - GAP) / 2.0).floor();
                    first.layout(Rect { w: w1, ..rect }, out);
                    second.layout(
                        Rect {
                            x: rect.x + w1 + GAP,
                            w: rect.w - w1 - GAP,
                            ..rect
                        },
                        out,
                    );
                }
                SplitDir::Column => {
                    let h1 = ((rect.h - GAP) / 2.0).floor();
                    first.layout(Rect { h: h1, ..rect }, out);
                    second.layout(
                        Rect {
                            y: rect.y + h1 + GAP,
                            h: rect.h - h1 - GAP,
                            ..rect
                        },
                        out,
                    );
                }
            },
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
