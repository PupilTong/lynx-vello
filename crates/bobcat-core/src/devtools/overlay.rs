//! The `Overlay` domain of the page session: the node highlight, which the
//! document's next commit draws, and inspect mode, which takes every input
//! event that reaches the main thread while it is on.

use serde::{Deserialize, Serialize};
use serde_json::json;
use terminus::link::peniko::Color;
use terminus::link::{NodeHighlight, NodeId};

use super::Frontend;
use super::protocol::{Error, Request};
use crate::link::InputEventPayload;
use crate::main::runtime::devtools::DevtoolsDocument;

const INSPECT_NAMES: [&str; 3] = ["pointerdown", "pointermove", "pointerup"];

#[derive(Default)]
pub(crate) struct OverlayState {
    inspect: Option<HighlightConfig>,
}

pub(crate) fn dispatch(
    state: &mut OverlayState,
    document: &mut DevtoolsDocument<'_>,
    frontend: &Frontend,
    request: &Request,
) {
    let reply = match request.method.as_str() {
        "Overlay.enable" => Ok(()),
        "Overlay.disable" => {
            detach(state, document);
            Ok(())
        }
        "Overlay.hideHighlight" => {
            hide(document);
            Ok(())
        }
        "Overlay.highlightNode" => request
            .params()
            .and_then(|params| highlight_node(document, &params)),
        "Overlay.setInspectMode" => request
            .params()
            .and_then(|params| set_inspect_mode(state, document, &params)),
        _ => return frontend.not_found(request),
    };
    frontend.reply(request.id, reply.map(|()| json!({})));
}

pub(crate) fn input_event(
    state: &mut OverlayState,
    document: &mut DevtoolsDocument<'_>,
    frontend: &Frontend,
    target: NodeId,
    name: &str,
    _payload: &InputEventPayload,
) -> bool {
    let Some(config) = state.inspect else {
        return false;
    };
    match name {
        "pointerdown" | "pointermove" => {
            if let Some(document) = document.document() {
                document.set_node_highlight(Some(config.highlight(target)));
            }
        }
        "pointerup" => frontend.event(
            "Overlay.inspectNodeRequested",
            &InspectNodeRequested {
                backend_node_id: target.to_bits(),
            },
        ),
        _ => {}
    }
    true
}

pub(crate) fn detach(state: &mut OverlayState, document: &mut DevtoolsDocument<'_>) {
    set_inspect(state, document, None);
    hide(document);
}

#[derive(Default)]
pub(crate) struct Routes {
    inspecting: bool,
}

impl Routes {
    #[inline]
    pub(crate) fn routes(&self, name: &str) -> bool {
        self.inspecting && INSPECT_NAMES.contains(&name)
    }
}

fn hide(document: &mut DevtoolsDocument<'_>) {
    if let Some(document) = document.document() {
        document.set_node_highlight(None);
    }
}

fn highlight_node(
    document: &mut DevtoolsDocument<'_>,
    params: &HighlightNode,
) -> Result<(), Error> {
    let id = params
        .node_id
        .or(params.backend_node_id)
        .ok_or_else(|| Error::invalid_params("Either nodeId or backendNodeId must be specified"))?;
    let document = document.document().ok_or_else(|| Error::server(NO_NODE))?;
    let node = NodeId::from_bits(id)
        .filter(|&node| document.get(node).is_some())
        .ok_or_else(|| Error::server(NO_NODE))?;
    document.set_node_highlight(Some(params.highlight_config.highlight(node)));
    Ok(())
}

const NO_NODE: &str = "Could not find node with given id";

fn set_inspect_mode(
    state: &mut OverlayState,
    document: &mut DevtoolsDocument<'_>,
    params: &SetInspectMode,
) -> Result<(), Error> {
    let inspect = match params.mode.as_str() {
        "searchForNode" | "searchForUAShadowDOM" => Some(params.highlight_config),
        "none" => None,
        mode => {
            return Err(Error::invalid_params(format!(
                "unsupported inspect mode `{mode}`"
            )));
        }
    };
    if inspect.is_none() && state.inspect.is_some() {
        hide(document);
    }
    set_inspect(state, document, inspect);
    Ok(())
}

fn set_inspect(
    state: &mut OverlayState,
    document: &mut DevtoolsDocument<'_>,
    inspect: Option<HighlightConfig>,
) {
    state.inspect = inspect;
    document.route_names(&INSPECT_NAMES, |routes| {
        routes.inspecting = inspect.is_some();
    });
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HighlightNode {
    highlight_config: HighlightConfig,
    node_id: Option<u64>,
    backend_node_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetInspectMode {
    mode: String,
    #[serde(default)]
    highlight_config: HighlightConfig,
}

#[derive(Clone, Copy, Default, Deserialize)]
struct HighlightConfig {
    #[serde(rename = "contentColor")]
    content: Option<Rgba>,
    #[serde(rename = "paddingColor")]
    padding: Option<Rgba>,
    #[serde(rename = "borderColor")]
    border: Option<Rgba>,
    #[serde(rename = "marginColor")]
    margin: Option<Rgba>,
}

impl HighlightConfig {
    fn highlight(self, node: NodeId) -> NodeHighlight {
        let color = |rgba: Option<Rgba>| rgba.map_or(Color::TRANSPARENT, Rgba::color);
        NodeHighlight {
            node,
            content: color(self.content),
            padding: color(self.padding),
            border: color(self.border),
            margin: color(self.margin),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    #[serde(default = "opaque")]
    a: f32,
}

const fn opaque() -> f32 {
    1.0
}

impl Rgba {
    fn color(self) -> Color {
        Color::from_rgb8(self.r, self.g, self.b).with_alpha(self.a.clamp(0.0, 1.0))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectNodeRequested {
    backend_node_id: u64,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
