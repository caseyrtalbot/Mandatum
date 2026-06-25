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
            return Ok(());
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutMutationError {
    PaneNotTiled(PaneId),
    CannotRemoveLastTiledPane,
    CannotFloatLastTiledPane,
    NoAdjacentPaneToStack,
}

impl fmt::Display for LayoutMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PaneNotTiled(pane_id) => write!(formatter, "pane {pane_id} is not tiled"),
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
