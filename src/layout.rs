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

pub enum PaneNode {
    /// 소유권 이동을 위한 일시적 상태 — 트리 조작 중에만 존재한다.
    Empty,
    Leaf(Pane),
    Split {
        dir: SplitDir,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    /// 주어진 사각형을 트리 구조대로 나눠 각 페인의 사각형을 계산한다.
    pub fn layout(&self, rect: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            PaneNode::Empty => {}
            PaneNode::Leaf(pane) => out.push((pane.id, rect)),
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

    pub fn panes(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            PaneNode::Empty => {}
            PaneNode::Leaf(pane) => out.push(pane),
            PaneNode::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    pub fn pane(&self, id: usize) -> Option<&Pane> {
        self.panes().into_iter().find(|p| p.id == id)
    }

    pub fn pane_mut(&mut self, id: usize) -> Option<&mut Pane> {
        match self {
            PaneNode::Empty => None,
            PaneNode::Leaf(pane) => (pane.id == id).then_some(pane),
            PaneNode::Split { first, second, .. } => {
                first.pane_mut(id).or_else(|| second.pane_mut(id))
            }
        }
    }

    pub fn first_id(&self) -> Option<usize> {
        self.panes().first().map(|p| p.id)
    }

    /// `target` 리프를 (기존, 새 페인)의 분할로 교체한다.
    /// 성공하면 None, 실패하면 새 페인을 되돌려준다.
    pub fn split_leaf(&mut self, target: usize, dir: SplitDir, new_pane: Pane) -> Option<Pane> {
        match self {
            PaneNode::Leaf(pane) if pane.id == target => {
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
            if matches!(&**first, PaneNode::Leaf(p) if p.id == target) {
                let sibling = std::mem::replace(&mut **second, PaneNode::Empty);
                *self = sibling;
                return true;
            }
            if matches!(&**second, PaneNode::Leaf(p) if p.id == target) {
                let sibling = std::mem::replace(&mut **first, PaneNode::Empty);
                *self = sibling;
                return true;
            }
            return first.remove(target) || second.remove(target);
        }
        false
    }
}
