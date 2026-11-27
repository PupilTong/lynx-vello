//! The style rules that matched an element, for a developer-tools backend:
//! read from the rule node chain of the element's last computed style, each
//! block mapped back to its style rule by pointer equality over a walk of
//! every rule in the `Stylist`'s sheets and in the shadow roots whose sheets
//! can apply to the element. Nothing is cached; each call repeats the walk.

use cssparser::ToCss;
use rustc_hash::FxHashMap;
use selectors::matching::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
    matches_selector,
};
use selectors::{Element, SelectorList};
use stylo::properties::{PropertyDeclarationBlock, PropertyDeclarationId};
use stylo::rule_tree::StyleSource;
use stylo::selector_parser::SelectorImpl;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLockReadGuard};
use stylo::stylesheets::{CssRule, Origin, StylesheetInDocument};

use crate::style::computed::is_author_facing;
use crate::style::engine::RuleGuards;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedStyles {
    pub inline: Option<Vec<MatchedDeclaration>>,
    pub presentational: Option<Vec<MatchedDeclaration>>,
    pub rules: Vec<MatchedRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedRule {
    pub origin: Origin,
    pub selectors: Vec<MatchedSelector>,
    pub declarations: Vec<MatchedDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedSelector {
    pub text: String,
    pub matches: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedDeclaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

type Block = Arc<Locked<PropertyDeclarationBlock>>;
type BlockKey = std::ptr::NonNull<()>;
type Selectors = SelectorList<SelectorImpl>;
type Visit<'v> = dyn FnMut(&Block, &Selectors, &dyn Fn() -> Selectors) + 'v;

struct FoundRule<'a, T> {
    origin: Origin,
    host: Option<&'a Node<T>>,
    written: Selectors,
    matching: Selectors,
}

impl<T: Sync> Document<T> {
    #[must_use]
    pub fn matched_styles(&self, element: NodeId) -> Option<MatchedStyles> {
        let node = self.get(element).filter(|node| node.is_element())?;
        let style = node.computed_style()?;
        let mut sources: Vec<&Block> = style
            .rules()
            .self_and_ancestors()
            .filter_map(|rule_node| rule_node.style_source().map(StyleSource::get))
            .collect();
        sources.reverse();
        let mut chain: Vec<Block> = Vec::new();
        for block in sources {
            if !chain.iter().any(|seen| Arc::ptr_eq(seen, block)) {
                chain.push(block.clone());
            }
        }
        let wanted: Vec<BlockKey> = chain.iter().map(Arc::raw_ptr).collect();

        let guards = self.style_engine().rule_guards();
        let author = guards.for_origin(Origin::Author);
        let found = self.rules_by_block(node, &wanted, &guards);

        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            self.quirks_mode(),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        context.scope_element = Some(Element::opaque(&node));
        let containing_host = Element::containing_shadow_host(&node);

        let same = |own: &Option<Block>, block: &Block| {
            own.as_ref().is_some_and(|own| Arc::ptr_eq(own, block))
        };
        let mut matched = MatchedStyles {
            inline: None,
            presentational: None,
            rules: Vec::new(),
        };
        for block in &chain {
            if same(&node.parsed_inline_style, block) {
                matched.inline = Some(declarations(block, author));
            } else if same(&node.presentational_hints, block) {
                matched.presentational = Some(declarations(block, author));
            } else if let Some(rule) = found.get(&block.raw_ptr()) {
                let host = rule.host.or(containing_host);
                let selectors = context.with_shadow_host(host, |context| {
                    rule.written
                        .slice()
                        .iter()
                        .zip(rule.matching.slice())
                        .map(|(written, matching)| MatchedSelector {
                            text: written.to_css_string(),
                            matches: matches_selector(matching, 0, None, node, context),
                        })
                        .collect()
                });
                matched.rules.push(MatchedRule {
                    origin: rule.origin,
                    selectors,
                    declarations: declarations(block, guards.for_origin(rule.origin)),
                });
            }
        }
        Some(matched)
    }

    fn rules_by_block<'a>(
        &'a self,
        node: &'a Node<T>,
        wanted: &[BlockKey],
        guards: &RuleGuards<'_>,
    ) -> FxHashMap<BlockKey, FoundRule<'a, T>> {
        let mut found = FxHashMap::default();
        let mut scan = |rules: &[CssRule], guard: &SharedRwLockReadGuard<'_>, origin, host| {
            collect(rules, guard, None, &mut |block, written, matching| {
                if wanted.contains(&block.raw_ptr()) {
                    found.entry(block.raw_ptr()).or_insert_with(|| FoundRule {
                        origin,
                        host,
                        written: written.clone(),
                        matching: matching(),
                    });
                }
            });
        };
        let stylist = self.style_engine().stylist();
        for origin in [Origin::UserAgent, Origin::User, Origin::Author] {
            let guard = guards.for_origin(origin);
            for index in 0..stylist.sheet_count(origin) {
                if let Some(sheet) = stylist.sheet_at(origin, index) {
                    scan(sheet.contents(guard).rules(guard), guard, origin, None);
                }
            }
        }
        let author = guards.for_origin(Origin::Author);
        let mut current = Some(node);
        while let Some(at) = current {
            if let Some(root) = at.shadow_root_id().and_then(|id| self.get(id))
                && let Some(data) = root.shadow_data()
            {
                let host = root.shadow_host_id().and_then(|id| self.get(id));
                for sheet in data.styles.stylesheets.iter() {
                    scan(
                        sheet.contents(author).rules(author),
                        author,
                        Origin::Author,
                        host,
                    );
                }
            }
            current = if at.is_shadow_root() {
                at.shadow_host_id().and_then(|id| self.get(id))
            } else {
                at.parent_id().and_then(|id| self.get(id))
            };
        }
        found
    }
}

fn collect<'a>(
    rules: &'a [CssRule],
    guard: &'a SharedRwLockReadGuard<'_>,
    parent: Option<(&Selectors, &Selectors)>,
    visit: &mut Visit<'_>,
) {
    for rule in rules {
        match rule {
            CssRule::Style(locked) => {
                let style = locked.read_with(guard);
                let matching = || match parent {
                    Some((_, ancestors)) => style.selectors.replace_parent_selector(ancestors),
                    None => style.selectors.clone(),
                };
                visit(&style.block, &style.selectors, &matching);
                if let Some(nested) = &style.rules {
                    let matching = matching();
                    collect(
                        &nested.read_with(guard).0,
                        guard,
                        Some((&style.selectors, &matching)),
                        visit,
                    );
                }
            }
            CssRule::NestedDeclarations(locked) => {
                if let Some((written, matching)) = parent {
                    visit(&locked.read_with(guard).block, written, &|| {
                        matching.clone()
                    });
                }
            }
            _ => collect(rule.children(guard), guard, parent, visit),
        }
    }
}

fn declarations(block: &Block, guard: &SharedRwLockReadGuard<'_>) -> Vec<MatchedDeclaration> {
    block
        .read_with(guard)
        .declaration_importance_iter()
        .filter(|(declaration, _)| match declaration.id() {
            PropertyDeclarationId::Longhand(longhand) => is_author_facing(longhand),
            PropertyDeclarationId::Custom(_) => true,
        })
        .map(|(declaration, importance)| {
            let mut value = String::new();
            let _ = declaration.to_css(&mut value);
            MatchedDeclaration {
                name: declaration.id().name().into_owned(),
                value,
                important: importance.important(),
            }
        })
        .collect()
}
