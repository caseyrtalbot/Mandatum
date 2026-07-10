use std::fmt;

use serde::{Deserialize, Serialize};

use crate::PaneId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    root: LayoutNode,
    floating: Vec<FloatingPane>,
    zoomed: Option<PaneId>,
}

impl Layout {
    pub fn new(root_pane: PaneId) -> Self {
        Self {
            root: LayoutNode::Pane { pane_id: root_pane },
            floating: Vec::new(),
            zoomed: None,
        }
    }

    pub fn root(&self) -> &LayoutNode {
        &self.root
    }

    pub fn floating(&self) -> &[FloatingPane] {
        &self.floating
    }

    pub fn zoomed(&self) -> Option<&PaneId> {
        self.zoomed.as_ref()
    }

    pub fn toggle_zoom(&mut self, pane_id: &PaneId) {
        if self.zoomed.as_ref() == Some(pane_id) {
            self.zoomed = None;
        } else {
            self.zoomed = Some(pane_id.clone());
        }
    }

    pub fn clear_zoom_if(&mut self, pane_id: &PaneId) {
        if self.zoomed.as_ref() == Some(pane_id) {
            self.zoomed = None;
        }
    }

    pub fn pane_order(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.root.pane_order(&mut panes);
        panes.extend(self.floating.iter().map(|pane| pane.pane_id.clone()));
        panes
    }

    pub fn tiled_pane_order(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.root.pane_order(&mut panes);
        panes
    }

    pub fn contains_pane(&self, pane_id: &PaneId) -> bool {
        self.root.contains(pane_id) || self.floating.iter().any(|pane| &pane.pane_id == pane_id)
    }

    pub fn is_floating(&self, pane_id: &PaneId) -> bool {
        self.floating.iter().any(|pane| &pane.pane_id == pane_id)
    }

    pub fn split_pane(
        &mut self,
        pane_id: &PaneId,
        new_pane_id: PaneId,
        direction: SplitDirection,
    ) -> Result<(), LayoutMutationError> {
        if self.root.split_target(pane_id, new_pane_id, direction) {
            Ok(())
        } else {
            Err(LayoutMutationError::PaneNotTiled(pane_id.clone()))
        }
    }

    pub fn add_floating(&mut self, pane_id: PaneId) {
        self.floating.push(FloatingPane {
            pane_id,
            rect: FloatingRect::default(),
        });
    }

    pub fn remove_pane(&mut self, pane_id: &PaneId) -> Result<(), LayoutMutationError> {
        if let Some(index) = self
            .floating
            .iter()
            .position(|floating| &floating.pane_id == pane_id)
        {
            self.floating.remove(index);
            self.clear_zoom_if(pane_id);
            return Ok(());
        }

        if self.tiled_pane_order().len() <= 1 {
            return Err(LayoutMutationError::CannotRemoveLastTiledPane);
        }

        let next_root = self
            .root
            .clone()
            .remove_pane(pane_id)
            .ok_or(LayoutMutationError::CannotRemoveLastTiledPane)?;
        self.root = next_root;
        self.clear_zoom_if(pane_id);
        Ok(())
    }

    pub fn float_pane(&mut self, pane_id: &PaneId) -> Result<(), LayoutMutationError> {
        if self.is_floating(pane_id) {
            // Floating a floating pane is not a success: reporting Ok would
            // tell the user something happened when nothing did.
            return Err(LayoutMutationError::PaneAlreadyFloating(pane_id.clone()));
        }

        if self.tiled_pane_order().len() <= 1 {
            return Err(LayoutMutationError::CannotFloatLastTiledPane);
        }

        let next_root = self
            .root
            .clone()
            .remove_pane(pane_id)
            .ok_or_else(|| LayoutMutationError::PaneNotTiled(pane_id.clone()))?;
        self.root = next_root;
        self.add_floating(pane_id.clone());
        self.clear_zoom_if(pane_id);
        Ok(())
    }

    /// Return a floating pane to the tiled tree: it becomes the second side
    /// of a new 50/50 horizontal split over the whole tiled area. The
    /// inverse of [`Layout::float_pane`].
    pub fn dock_pane(&mut self, pane_id: &PaneId) -> Result<(), LayoutMutationError> {
        let index = self
            .floating
            .iter()
            .position(|floating| &floating.pane_id == pane_id)
            .ok_or_else(|| LayoutMutationError::PaneNotFloating(pane_id.clone()))?;
        self.floating.remove(index);

        let existing = std::mem::replace(
            &mut self.root,
            LayoutNode::Pane {
                pane_id: pane_id.clone(),
            },
        );
        self.root = LayoutNode::Split {
            axis: SplitAxis::Horizontal,
            first_percent: 50,
            first: Box::new(existing),
            second: Box::new(LayoutNode::Pane {
                pane_id: pane_id.clone(),
            }),
        };
        Ok(())
    }

    /// Grow (`delta` > 0) or shrink (`delta` < 0) a tiled pane's share of
    /// its nearest enclosing split by `delta` percentage points, clamped to
    /// 1..=99 so neither side collapses. The keyboard counterpart of
    /// dragging the pane's split separator.
    pub fn resize_pane(&mut self, pane_id: &PaneId, delta: i8) -> Result<(), LayoutMutationError> {
        if !self.root.contains(pane_id) {
            return Err(LayoutMutationError::PaneNotTiled(pane_id.clone()));
        }
        if self.root.resize_pane(pane_id, i16::from(delta)) {
            Ok(())
        } else {
            Err(LayoutMutationError::NoSplitToResize)
        }
    }

    /// Set the first-side percentage of the `split_index`-th split node in
    /// preorder (the order [`LayoutNode`] children are visited: node, first
    /// subtree, second subtree). The percentage is clamped to 1..=99 so
    /// neither side collapses to nothing.
    pub fn set_split_percent(
        &mut self,
        split_index: usize,
        first_percent: u8,
    ) -> Result<(), LayoutMutationError> {
        let mut next_index = 0;
        if self
            .root
            .set_split_percent(split_index, &mut next_index, first_percent.clamp(1, 99))
        {
            Ok(())
        } else {
            Err(LayoutMutationError::SplitNotFound(split_index))
        }
    }

    /// Move a floating pane's top-left corner (coordinates relative to the
    /// workspace area, like the rest of [`FloatingRect`]).
    pub fn set_floating_position(
        &mut self,
        pane_id: &PaneId,
        x: u16,
        y: u16,
    ) -> Result<(), LayoutMutationError> {
        let floating = self
            .floating
            .iter_mut()
            .find(|floating| &floating.pane_id == pane_id)
            .ok_or_else(|| LayoutMutationError::PaneNotFloating(pane_id.clone()))?;
        floating.rect.x = x;
        floating.rect.y = y;
        Ok(())
    }

    pub fn stack_with_next(&mut self, pane_id: &PaneId) -> Result<PaneId, LayoutMutationError> {
        let order = self.tiled_pane_order();
        let focused_index = order
            .iter()
            .position(|candidate| candidate == pane_id)
            .ok_or_else(|| LayoutMutationError::PaneNotTiled(pane_id.clone()))?;

        if order.len() < 2 {
            return Err(LayoutMutationError::NoAdjacentPaneToStack);
        }

        let adjacent = order[(focused_index + 1) % order.len()].clone();
        let next_root = self
            .root
            .clone()
            .remove_pane(&adjacent)
            .ok_or(LayoutMutationError::NoAdjacentPaneToStack)?;
        self.root = next_root;

        let stack = LayoutNode::Stack {
            active: pane_id.clone(),
            panes: vec![pane_id.clone(), adjacent.clone()],
        };
        if self.root.replace_pane_with_stack(pane_id, stack) {
            Ok(adjacent)
        } else {
            Err(LayoutMutationError::PaneNotTiled(pane_id.clone()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Pane {
        pane_id: PaneId,
    },
    Split {
        axis: SplitAxis,
        first_percent: u8,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
    Stack {
        active: PaneId,
        panes: Vec<PaneId>,
    },
}

impl LayoutNode {
    fn pane_order(&self, panes: &mut Vec<PaneId>) {
        match self {
            Self::Pane { pane_id } => panes.push(pane_id.clone()),
            Self::Split { first, second, .. } => {
                first.pane_order(panes);
                second.pane_order(panes);
            }
            Self::Stack { panes: stack, .. } => panes.extend(stack.iter().cloned()),
        }
    }

    fn contains(&self, pane_id: &PaneId) -> bool {
        match self {
            Self::Pane { pane_id: current } => current == pane_id,
            Self::Split { first, second, .. } => {
                first.contains(pane_id) || second.contains(pane_id)
            }
            Self::Stack { panes, .. } => panes.iter().any(|current| current == pane_id),
        }
    }

    fn split_target(
        &mut self,
        target: &PaneId,
        new_pane_id: PaneId,
        direction: SplitDirection,
    ) -> bool {
        match self {
            Self::Pane { pane_id } if pane_id == target => {
                let existing = std::mem::replace(
                    self,
                    Self::Pane {
                        pane_id: new_pane_id.clone(),
                    },
                );
                *self = Self::Split {
                    axis: direction.axis(),
                    first_percent: 50,
                    first: Box::new(existing),
                    second: Box::new(Self::Pane {
                        pane_id: new_pane_id,
                    }),
                };
                true
            }
            Self::Pane { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_target(target, new_pane_id.clone(), direction)
                    || second.split_target(target, new_pane_id, direction)
            }
            Self::Stack { .. } if self.contains(target) => {
                let existing = std::mem::replace(
                    self,
                    Self::Pane {
                        pane_id: new_pane_id.clone(),
                    },
                );
                *self = Self::Split {
                    axis: direction.axis(),
                    first_percent: 50,
                    first: Box::new(existing),
                    second: Box::new(Self::Pane {
                        pane_id: new_pane_id,
                    }),
                };
                true
            }
            Self::Stack { .. } => false,
        }
    }

    fn remove_pane(self, target: &PaneId) -> Option<Self> {
        match self {
            Self::Pane { pane_id } if &pane_id == target => None,
            Self::Pane { pane_id } => Some(Self::Pane { pane_id }),
            Self::Split {
                axis,
                first_percent,
                first,
                second,
            } => {
                let first = first.remove_pane(target);
                let second = second.remove_pane(target);

                match (first, second) {
                    (Some(first), Some(second)) => Some(Self::Split {
                        axis,
                        first_percent,
                        first: Box::new(first),
                        second: Box::new(second),
                    }),
                    (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                    (None, None) => None,
                }
            }
            Self::Stack { active, mut panes } => {
                panes.retain(|pane_id| pane_id != target);
                match panes.len() {
                    0 => None,
                    1 => Some(Self::Pane {
                        pane_id: panes.remove(0),
                    }),
                    _ => {
                        let active = if active == *target {
                            panes[0].clone()
                        } else {
                            active
                        };
                        Some(Self::Stack { active, panes })
                    }
                }
            }
        }
    }

    fn set_split_percent(&mut self, target: usize, next_index: &mut usize, percent: u8) -> bool {
        match self {
            Self::Pane { .. } | Self::Stack { .. } => false,
            Self::Split {
                first_percent,
                first,
                second,
                ..
            } => {
                let index = *next_index;
                *next_index += 1;
                if index == target {
                    *first_percent = percent;
                    return true;
                }
                first.set_split_percent(target, next_index, percent)
                    || second.set_split_percent(target, next_index, percent)
            }
        }
    }

    /// Adjust the nearest (deepest) enclosing split of `target` by `delta`
    /// points toward the side that holds it. Returns whether any split was
    /// adjusted.
    fn resize_pane(&mut self, target: &PaneId, delta: i16) -> bool {
        let Self::Split {
            first_percent,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        if first.contains(target) {
            if !first.resize_pane(target, delta) {
                *first_percent = clamp_split_percent(i16::from(*first_percent) + delta);
            }
            true
        } else if second.contains(target) {
            if !second.resize_pane(target, delta) {
                *first_percent = clamp_split_percent(i16::from(*first_percent) - delta);
            }
            true
        } else {
            false
        }
    }

    fn replace_pane_with_stack(&mut self, target: &PaneId, stack: Self) -> bool {
        match self {
            Self::Pane { pane_id } if pane_id == target => {
                *self = stack;
                true
            }
            Self::Pane { .. } => false,
            Self::Split { first, second, .. } => {
                first.replace_pane_with_stack(target, stack.clone())
                    || second.replace_pane_with_stack(target, stack)
            }
            Self::Stack { active, panes } if panes.iter().any(|pane_id| pane_id == target) => {
                if let Self::Stack {
                    active: new_active,
                    panes: new_panes,
                } = stack
                {
                    *active = new_active;
                    for pane_id in new_panes {
                        if !panes.contains(&pane_id) {
                            panes.push(pane_id);
                        }
                    }
                }
                true
            }
            Self::Stack { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    fn axis(self) -> SplitAxis {
        match self {
            Self::Right => SplitAxis::Horizontal,
            Self::Down => SplitAxis::Vertical,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatingPane {
    pub pane_id: PaneId,
    pub rect: FloatingRect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatingRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Default for FloatingRect {
    fn default() -> Self {
        Self {
            x: 8,
            y: 4,
            width: 96,
            height: 28,
        }
    }
}

fn clamp_split_percent(value: i16) -> u8 {
    value.clamp(1, 99) as u8
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutMutationError {
    PaneNotTiled(PaneId),
    PaneNotFloating(PaneId),
    PaneAlreadyFloating(PaneId),
    SplitNotFound(usize),
    NoSplitToResize,
    CannotRemoveLastTiledPane,
    CannotFloatLastTiledPane,
    NoAdjacentPaneToStack,
}

impl fmt::Display for LayoutMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PaneNotTiled(pane_id) => write!(formatter, "pane {pane_id} is not tiled"),
            Self::PaneNotFloating(pane_id) => write!(formatter, "pane {pane_id} is not floating"),
            Self::PaneAlreadyFloating(pane_id) => write!(
                formatter,
                "pane {pane_id} is already floating (Dock pane tiles it again)"
            ),
            Self::NoSplitToResize => {
                formatter.write_str("no split to resize: split the pane first")
            }
            Self::SplitNotFound(split_index) => {
                write!(formatter, "layout has no split {split_index}")
            }
            Self::CannotRemoveLastTiledPane => {
                formatter.write_str("cannot remove the last tiled pane")
            }
            Self::CannotFloatLastTiledPane => {
                formatter.write_str("cannot float the last tiled pane")
            }
            Self::NoAdjacentPaneToStack => {
                formatter.write_str("no adjacent tiled pane is available to stack")
            }
        }
    }
}

impl std::error::Error for LayoutMutationError {}
