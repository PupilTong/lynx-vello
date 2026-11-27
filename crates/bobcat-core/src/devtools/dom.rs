//! The `DOM` domain of the page session: node descriptions, the set of nodes
//! the frontend was told about, and the change events found by comparing
//! that set with the document after a commit.

#[cfg(test)]
mod tests;

use std::borrow::Cow;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use terminus::link::{NodeId, Point2D, ShadowRootMode};

use super::Frontend;
use super::protocol::{Error, Request};
use crate::main::tree::{LynxDocument, lynx_tag};

pub(crate) const FRAME_ID: &str = "page";

type Node = terminus::link::Node<()>;

#[derive(Default)]
pub(crate) struct DomState(Option<Known>);

#[derive(Default)]
struct Known(FxHashMap<NodeId, Entry>);

enum Entry {
    Text(String),
    Parent {
        attributes: Vec<(String, String)>,
        shadow: Option<NodeId>,
        children: Children,
    },
}

enum Children {
    Count(usize),
    Sent(Vec<NodeId>),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Description<'d> {
    node_id: u64,
    backend_node_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<u64>,
    node_type: u8,
    node_name: &'d str,
    local_name: &'d str,
    node_value: &'d str,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_node_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Description<'d>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes: Option<Vec<Cow<'d, str>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    shadow_roots: Vec<Description<'d>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shadow_root_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<&'static str>,
}

#[derive(Deserialize)]
struct TreeParams {
    depth: Option<i64>,
    #[serde(default)]
    pierce: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChildParams {
    node_id: u64,
    #[serde(flatten)]
    tree: TreeParams,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackendIds {
    backend_node_ids: Vec<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoxParams {
    node_id: Option<u64>,
    backend_node_id: Option<u64>,
}

#[derive(Deserialize)]
struct Location {
    x: f32,
    y: f32,
}

pub(crate) fn dispatch(
    state: &mut DomState,
    document: &mut LynxDocument,
    frontend: &Frontend,
    request: &Request,
) {
    let document: &LynxDocument = document;
    let id = request.id;
    match request.method.as_str() {
        "DOM.enable" => {
            state.0.get_or_insert_default();
            frontend.result(id, &json!({}));
        }
        "DOM.disable" => {
            state.0 = None;
            frontend.result(id, &json!({}));
        }
        "DOM.getDocument" => frontend.reply(
            id,
            request.params().and_then(|params: TreeParams| {
                let depth = depth(params.depth)?;
                let known = state.0.insert(Known::default());
                let mut root = known.describe(document, document.root_node(), depth, params.pierce);
                root.frame_id = Some(FRAME_ID);
                Ok(json!({ "root": root }))
            }),
        ),
        "DOM.requestChildNodes" => frontend.reply(
            id,
            request.params().and_then(|params: ChildParams| {
                let known = state
                    .0
                    .as_mut()
                    .filter(|known| {
                        NodeId::from_bits(params.node_id)
                            .is_some_and(|node| known.0.contains_key(&node))
                    })
                    .ok_or_else(no_node)?;
                let node = find(document, params.node_id).ok_or_else(no_node)?;
                known.push_children(
                    document,
                    frontend,
                    node,
                    depth(params.tree.depth)?,
                    params.tree.pierce,
                );
                Ok(json!({}))
            }),
        ),
        "DOM.pushNodesByBackendIdsToFrontend" => frontend.reply(
            id,
            request.params().and_then(|params: BackendIds| {
                let root = document.root_node().id();
                let known = state
                    .0
                    .as_mut()
                    .filter(|known| known.0.contains_key(&root))
                    .ok_or_else(|| Error::server("Document needs to be requested first"))?;
                let node_ids: Vec<u64> = params
                    .backend_node_ids
                    .iter()
                    .map(|&bits| known.push_path(document, frontend, bits))
                    .collect();
                Ok(json!({ "nodeIds": node_ids }))
            }),
        ),
        "DOM.getBoxModel" => frontend.reply(
            id,
            request
                .params()
                .and_then(|params: BoxParams| box_model(document, &params)),
        ),
        "DOM.getNodeForLocation" => frontend.reply(
            id,
            request.params().and_then(|params: Location| {
                let mut node = document
                    .elements_from_point(Point2D::new(params.x, params.y))
                    .first()
                    .and_then(|&id| document.get(id))
                    .ok_or_else(|| Error::server("No node found at given location"))?;
                while node.is_text_node()
                    && let Some(parent) = node.parent_id().and_then(|id| document.get(id))
                {
                    node = parent;
                }
                let id = node.id();
                let mut answer = json!({ "backendNodeId": id.to_bits(), "frameId": FRAME_ID });
                if state
                    .0
                    .as_ref()
                    .is_some_and(|known| known.0.contains_key(&id))
                {
                    answer["nodeId"] = json!(id.to_bits());
                }
                Ok(answer)
            }),
        ),
        _ => frontend.not_found(request),
    }
}

pub(crate) fn document_changed(
    state: &mut DomState,
    document: &mut LynxDocument,
    frontend: &Frontend,
) {
    let root = document.root_node().id();
    if let Some(known) = state.0.as_mut().filter(|known| known.0.contains_key(&root)) {
        known.remove_changed(document, frontend, root);
        known.insert_changed(document, frontend, root);
    }
}

pub(super) fn no_node() -> Error {
    Error::server("Could not find node with given id")
}

pub(super) fn find(document: &LynxDocument, bits: u64) -> Option<&Node> {
    let id = NodeId::from_bits(bits)?;
    document.get(id).filter(|_| document.is_connected(id))
}

fn depth(depth: Option<i64>) -> Result<u32, Error> {
    match depth.unwrap_or(1) {
        -1 => Ok(u32::MAX),
        depth => u32::try_from(depth)
            .ok()
            .filter(|&depth| depth > 0)
            .ok_or_else(|| {
                Error::server(
                    "Please provide a positive integer as a depth or -1 for entire subtree",
                )
            }),
    }
}

fn box_model(document: &LynxDocument, params: &BoxParams) -> Result<serde_json::Value, Error> {
    let node = match (params.node_id, params.backend_node_id) {
        (Some(bits), _) => find(document, bits).ok_or_else(no_node)?,
        (None, Some(bits)) => find(document, bits)
            .ok_or_else(|| Error::server("No node found for given backend id"))?,
        (None, None) => {
            return Err(Error::server(
                "Either nodeId or backendNodeId must be specified",
            ));
        }
    };
    let rect = document
        .bounding_client_rect(node.id())
        .ok_or_else(|| Error::server("Could not compute box model."))?;
    let [border, padding, margin] =
        document
            .rounded_layout(node.id())
            .map_or([[0.0; 4]; 3], |layout| {
                [layout.border, layout.padding, layout.margin]
                    .map(|edges| [edges.left, edges.top, edges.right, edges.bottom])
            });
    let border_box = [rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y()];
    let inset = |sides: [f32; 4], by: [f32; 4], sign: f32| {
        [
            sides[0] + sign * by[0],
            sides[1] + sign * by[1],
            sides[2] - sign * by[2],
            sides[3] - sign * by[3],
        ]
    };
    let padding_box = inset(border_box, border, 1.0);
    let quad =
        |[left, top, right, bottom]: [f32; 4]| [left, top, right, top, right, bottom, left, bottom];
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a laid-out box size in CSS px, far inside i64; `as` saturates"
    )]
    let [width, height] = [rect.width(), rect.height()].map(|size| size.round() as i64);
    Ok(json!({ "model": {
        "content": quad(inset(padding_box, padding, 1.0)),
        "padding": quad(padding_box),
        "border": quad(border_box),
        "margin": quad(inset(border_box, margin, -1.0)),
        "width": width,
        "height": height,
    }}))
}

impl Known {
    fn describe<'d>(
        &mut self,
        document: &'d LynxDocument,
        node: &'d Node,
        depth: u32,
        pierce: bool,
    ) -> Description<'d> {
        let id = node.id();
        let (node_type, node_name) = if node.is_element() {
            (1, lynx_tag(node.tag_name().unwrap_or_default()))
        } else if node.is_text_node() {
            (3, "#text")
        } else if node.is_document() {
            (9, "#document")
        } else {
            (11, "#document-fragment")
        };
        let mut description = Description {
            node_id: id.to_bits(),
            backend_node_id: id.to_bits(),
            parent_id: node.parent_id().map(NodeId::to_bits),
            node_type,
            node_name,
            local_name: if node.is_element() { node_name } else { "" },
            node_value: node.text().unwrap_or_default(),
            child_node_count: None,
            children: None,
            attributes: None,
            shadow_roots: Vec::new(),
            shadow_root_type: document.shadow_root_mode(id).map(|mode| match mode {
                ShadowRootMode::Open => "open",
                ShadowRootMode::Closed => "closed",
            }),
            frame_id: None,
        };
        if node.is_text_node() {
            self.0
                .insert(id, Entry::Text(description.node_value.to_owned()));
            return description;
        }
        let attributes: Vec<(&str, Cow<'d, str>)> = node.attributes().collect();
        let shadow = document.shadow_root(id).and_then(|root| document.get(root));
        description.shadow_roots = shadow
            .map(|root| self.describe(document, root, if pierce { depth } else { 0 }, pierce))
            .into_iter()
            .collect();
        let depth = if shadow.is_some() {
            depth.max(1)
        } else {
            depth
        };
        let child_ids = node.child_ids();
        description.child_node_count = Some(child_ids.len());
        let only_text =
            matches!(child_ids, [only] if document.get(*only).is_some_and(Node::is_text_node));
        let children = if depth > 0 || only_text {
            description.children =
                Some(self.describe_children(document, node, depth.saturating_sub(1), pierce));
            Children::Sent(child_ids.to_vec())
        } else {
            Children::Count(child_ids.len())
        };
        self.0.insert(
            id,
            Entry::Parent {
                attributes: attributes
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), value.clone().into_owned()))
                    .collect(),
                shadow: shadow.map(Node::id),
                children,
            },
        );
        if node.is_element() {
            description.attributes = Some(
                attributes
                    .into_iter()
                    .flat_map(|(name, value)| [Cow::Borrowed(name), value])
                    .collect(),
            );
        }
        description
    }

    fn describe_children<'d>(
        &mut self,
        document: &'d LynxDocument,
        node: &'d Node,
        depth: u32,
        pierce: bool,
    ) -> Vec<Description<'d>> {
        node.child_ids()
            .iter()
            .filter_map(|&child| document.get(child))
            .map(|child| self.describe(document, child, depth, pierce))
            .collect()
    }

    fn push_children(
        &mut self,
        document: &LynxDocument,
        frontend: &Frontend,
        node: &Node,
        depth: u32,
        pierce: bool,
    ) {
        let id = node.id();
        match self.0.get_mut(&id) {
            Some(Entry::Parent {
                children: Children::Sent(sent),
                ..
            }) => {
                if depth > 1 {
                    for child in sent.clone() {
                        if let Some(child) = document.get(child) {
                            self.push_children(document, frontend, child, depth - 1, pierce);
                        }
                    }
                }
            }
            Some(Entry::Parent { children, .. }) => {
                *children = Children::Sent(node.child_ids().to_vec());
                let nodes = self.describe_children(document, node, depth - 1, pierce);
                frontend.event(
                    "DOM.setChildNodes",
                    &json!({ "parentId": id.to_bits(), "nodes": nodes }),
                );
            }
            Some(Entry::Text(_)) | None => {}
        }
    }

    fn push_path(&mut self, document: &LynxDocument, frontend: &Frontend, bits: u64) -> u64 {
        let Some(node) = find(document, bits) else {
            return 0;
        };
        let mut path = Vec::new();
        let mut at = node;
        while !self.0.contains_key(&at.id()) {
            let Some(parent) = at.parent_id().and_then(|id| document.get(id)) else {
                return 0;
            };
            path.push(parent);
            at = parent;
        }
        for ancestor in path.into_iter().rev() {
            self.push_children(document, frontend, ancestor, 1, false);
        }
        if self.0.contains_key(&node.id()) {
            bits
        } else {
            0
        }
    }

    fn forget(&mut self, id: NodeId) {
        if let Some(entry) = self.0.remove(&id) {
            for child in descendants(&entry) {
                self.forget(child);
            }
        }
    }

    fn remove_changed(&mut self, document: &LynxDocument, frontend: &Frontend, id: NodeId) {
        let (Some(node), Some(mut entry)) = (document.get(id), self.0.remove(&id)) else {
            return;
        };
        let bits = id.to_bits();
        match &mut entry {
            Entry::Text(data) => {
                let live = node.text().unwrap_or_default();
                if data != live {
                    live.clone_into(data);
                    frontend.event(
                        "DOM.characterDataModified",
                        &json!({ "nodeId": bits, "characterData": live }),
                    );
                }
            }
            Entry::Parent {
                attributes,
                shadow,
                children,
            } => {
                attributes.retain(|(name, _)| {
                    let kept = node.attribute(name).is_some();
                    if !kept {
                        frontend.event(
                            "DOM.attributeRemoved",
                            &json!({ "nodeId": bits, "name": name }),
                        );
                    }
                    kept
                });
                for (name, value) in node.attributes() {
                    match attributes.iter_mut().find(|(known, _)| known == name) {
                        Some((_, known)) if *known == *value => continue,
                        Some((_, known)) => value.as_ref().clone_into(known),
                        None => attributes.push((name.to_owned(), value.clone().into_owned())),
                    }
                    frontend.event(
                        "DOM.attributeModified",
                        &json!({ "nodeId": bits, "name": name, "value": value }),
                    );
                }
                if let Some(root) = *shadow
                    && document.shadow_root(id) != Some(root)
                {
                    frontend.event(
                        "DOM.shadowRootPopped",
                        &json!({ "hostId": bits, "rootId": root.to_bits() }),
                    );
                    self.forget(root);
                    *shadow = None;
                }
                let live = node.child_ids();
                match children {
                    Children::Count(count) => {
                        if *count != live.len() {
                            *count = live.len();
                            frontend.event(
                                "DOM.childNodeCountUpdated",
                                &json!({ "nodeId": bits, "childNodeCount": count }),
                            );
                        }
                    }
                    Children::Sent(sent) if sent.as_slice() == live => {}
                    Children::Sent(sent) => {
                        let positions: FxHashMap<NodeId, usize> = live
                            .iter()
                            .enumerate()
                            .map(|(at, &child)| (child, at))
                            .collect();
                        let mut last = None;
                        sent.retain(|child| {
                            let at = positions.get(child).copied();
                            (at.is_some() && at > last && {
                                last = at;
                                true
                            }) || {
                                frontend.event(
                                    "DOM.childNodeRemoved",
                                    &json!({ "parentNodeId": bits, "nodeId": child.to_bits() }),
                                );
                                self.forget(*child);
                                false
                            }
                        });
                    }
                }
            }
        }
        let descend = descendants(&entry);
        self.0.insert(id, entry);
        for child in descend {
            self.remove_changed(document, frontend, child);
        }
    }

    fn insert_changed(&mut self, document: &LynxDocument, frontend: &Frontend, id: NodeId) {
        let (Some(node), Some(mut entry)) = (document.get(id), self.0.remove(&id)) else {
            return;
        };
        let bits = id.to_bits();
        let descend = descendants(&entry);
        if let Entry::Parent {
            shadow, children, ..
        } = &mut entry
        {
            if shadow.is_none()
                && let Some(root) = document.shadow_root(id).and_then(|root| document.get(root))
            {
                *shadow = Some(root.id());
                let root = self.describe(document, root, 0, false);
                frontend.event(
                    "DOM.shadowRootPushed",
                    &json!({ "hostId": bits, "root": root }),
                );
            }
            let live = node.child_ids();
            if let Children::Sent(sent) = children
                && sent.as_slice() != live
            {
                let mut previous = 0;
                for &child in live {
                    if !self.0.contains_key(&child)
                        && let Some(child) = document.get(child)
                    {
                        let child = self.describe(document, child, 0, false);
                        frontend.event(
                            "DOM.childNodeInserted",
                            &json!({ "parentNodeId": bits, "previousNodeId": previous, "node": child }),
                        );
                    }
                    previous = child.to_bits();
                }
                *sent = live.to_vec();
            }
        }
        self.0.insert(id, entry);
        for child in descend {
            self.insert_changed(document, frontend, child);
        }
    }
}

fn descendants(entry: &Entry) -> Vec<NodeId> {
    match entry {
        Entry::Parent {
            shadow,
            children: Children::Sent(sent),
            ..
        } => sent.iter().copied().chain(*shadow).collect(),
        Entry::Parent { shadow, .. } => shadow.iter().copied().collect(),
        Entry::Text(_) => Vec::new(),
    }
}
