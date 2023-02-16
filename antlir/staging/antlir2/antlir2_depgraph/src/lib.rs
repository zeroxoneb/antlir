/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

#![feature(io_error_other)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fmt::Display;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use buck_label::Label;
use features::Feature;
use itertools::Itertools;
use petgraph::graph::DefaultIx;
use petgraph::graph::DiGraph;
use petgraph::graph::NodeIndex;
use petgraph::visit::Dfs;
use petgraph::Direction;
use serde::Deserialize;
use serde::Serialize;
use serde_with::serde_as;
use strum::IntoEnumIterator;

mod dot;
mod item;
use item::Item;
use item::ItemKey;
mod phase;
pub use phase::Phase;
mod requires_provides;
use requires_provides::FeatureExt as _;
use requires_provides::Validator;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub enum Node<'a> {
    /// A Feature that is to be compiled in this layer.
    PendingFeature(Feature<'a>),
    /// An item provided by the parent layer or a feature in this layer.
    Item(Item<'a>),
    /// An item that is required by a feature, but does not exist in the graph.
    /// Distinct from a [Node::Item] without any [Edge::Provides] edges because
    /// not enough information is known about missing dependencies to construct
    /// the full [Item].
    MissingItem(ItemKey<'a>),
    /// A Feature that was provided by a layer in the parent chain.
    ParentFeature(Feature<'a>),
    /// Start of a distinct phase of the image build process.
    PhaseStart(Phase),
    /// End of a distinct phase of the image build process. All features that
    /// are part of that phase will have an edge pointing to the end.
    PhaseEnd(Phase),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Edge {
    /// This feature is part of a phase
    PartOf,
    /// This feature provides an item
    Provides,
    /// This feature requires a provided item, and requires additional
    /// validation
    Requires(Validator),
    /// Simple ordering edge that does not require any additional checks
    After,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent, bound(deserialize = "'de: 'a"))]
pub struct Cycle<'a>(Vec<Feature<'a>>);

impl<'a> Display for Cycle<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for feature in &self.0 {
            writeln!(f, "  {feature:?}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", bound(deserialize = "'de: 'a"))]
pub enum Error<'a> {
    #[error("cycle in dependency graph:\n{0}")]
    Cycle(Cycle<'a>),
    #[error("{item:?} is provided by multiple features: {features:#?}")]
    Conflict {
        item: Item<'a>,
        features: BTreeSet<Feature<'a>>,
    },
    #[error("{key:?} is required but was never provided")]
    MissingItem { key: ItemKey<'a> },
    #[error("{item:?} does not satisfy the validation rules: {validator:?}")]
    Unsatisfied {
        item: Item<'a>,
        validator: Validator,
    },
}

pub type Result<'a, T> = std::result::Result<T, Error<'a>>;

#[derive(Debug)]
pub struct GraphBuilder<'a> {
    g: DiGraph<Node<'a>, Edge, DefaultIx>,
    root: NodeIndex<DefaultIx>,
    pending_features: Vec<NodeIndex<DefaultIx>>,
    items: HashMap<ItemKey<'a>, NodeIndex<DefaultIx>>,
    phases: BTreeMap<Phase, (NodeIndex<DefaultIx>, NodeIndex<DefaultIx>)>,
}

impl<'a> Default for GraphBuilder<'a> {
    fn default() -> Self {
        let mut g = DiGraph::new();
        let mut items = HashMap::new();

        let phases: BTreeMap<_, _> = Phase::iter()
            .map(|p| {
                (
                    p,
                    (
                        g.add_node(Node::PhaseStart(p)),
                        g.add_node(Node::PhaseEnd(p)),
                    ),
                )
            })
            .collect();

        let root = phases[&Phase::Init].0;

        // Set up ordering for phases
        for ((a_start, a_end), (b_start, b_end)) in phases.values().tuple_windows() {
            g.update_edge(*a_start, *a_end, Edge::After);
            g.update_edge(*a_end, *b_start, Edge::After);
            g.update_edge(*b_start, *b_end, Edge::After);
        }

        // Some items are always available, since they are a property of the
        // operating system. Add them to the graph now so that the dependency
        // checks will be satisfied.
        for item in [
            Item::User(item::User {
                name: "root".into(),
            }),
            Item::Group(item::Group {
                name: "root".into(),
            }),
            Item::Path(item::Path::Entry(item::FsEntry {
                path: Path::new("/").into(),
                file_type: item::FileType::Directory,
                mode: 0o0755,
            })),
        ] {
            let key = item.key();
            let nx = g.add_node(Node::Item(item));
            items.insert(key, nx);
        }

        Self {
            g,
            root,
            pending_features: Vec::new(),
            items,
            phases,
        }
    }
}

impl<'a> GraphBuilder<'a> {
    pub fn new(parent: Option<Graph<'a>>) -> Self {
        let mut s = Self::default();
        if let Some(parent) = parent {
            let mut new_nodes = HashMap::new();
            for nx in parent.g.node_indices() {
                let new_nx = match &parent.g[nx] {
                    Node::Item(i) => Some(s.add_item(i.clone())),
                    Node::PendingFeature(f) | Node::ParentFeature(f) => {
                        Some(s.g.add_node(Node::ParentFeature(f.clone())))
                    }
                    _ => None,
                };
                if let Some(new_nx) = new_nx {
                    new_nodes.insert(nx, new_nx);
                }
            }
            for ex in parent.g.edge_indices() {
                let (a, b) = parent.g.edge_endpoints(ex).expect("must exist");
                let weight = parent.g.edge_weight(ex).expect("must exist").clone();
                if new_nodes.contains_key(&a) && new_nodes.contains_key(&b) {
                    s.g.add_edge(new_nodes[&a], new_nodes[&b], weight);
                }
            }
        }
        s
    }

    fn add_item(&mut self, item: Item<'a>) -> NodeIndex<DefaultIx> {
        let key = item.key();
        // If this Item undos another, it needs to be added to the graph on its
        // own. The previous Item will be left in the graph, but will be
        // overwritten in the items tracker map to this new node which is the
        // most recent version of that item
        if self.items.contains_key(&key) && item.is_undo() {
            let nx = self.g.add_node(Node::Item(item));
            self.g
                .add_edge(self.items[&key], nx, Edge::Requires(Validator::Exists));
            self.items.insert(key, nx);
            nx
        } else {
            *self
                .items
                .entry(key)
                .or_insert_with(|| self.g.add_node(Node::Item(item)))
        }
    }

    pub fn add_layer_dependency(&mut self, label: Label<'a>, graph: Graph<'a>) -> &mut Self {
        self.add_item(Item::Layer(item::Layer { label, graph }));
        self
    }

    pub fn add_feature(&mut self, feature: Feature<'a>) -> &mut Self {
        let provides = feature.provides();

        let phase = Phase::for_feature(&feature);

        let feature_nx = self.g.add_node(Node::PendingFeature(feature));
        self.pending_features.push(feature_nx);

        // Insert edges so that features are in the right part of the graph wrt phases
        self.g
            .update_edge(self.phases[&phase].0, feature_nx, Edge::PartOf);
        self.g
            .update_edge(feature_nx, self.phases[&phase].1, Edge::After);

        for prov in provides {
            let prov_nx = self.add_item(prov);
            self.g.update_edge(feature_nx, prov_nx, Edge::Provides);
        }

        self
    }

    pub fn to_dot<'b>(&'b self) -> dot::Dot<'a, 'b> {
        dot::Dot(&self.g)
    }

    pub fn build(mut self) -> Result<'a, Graph<'a>> {
        // Add all the requirements edges after all provided items are added so
        // that we know if a required item is missing or just not encountered
        // yet
        for feature_nx in &self.pending_features {
            if let Node::PendingFeature(f) = &self.g[*feature_nx] {
                for req in f.requires() {
                    let req_nx = match self.items.get(&req.key) {
                        Some(nx) => *nx,
                        None => {
                            let nx = self.g.add_node(Node::MissingItem(req.key.clone()));
                            self.items.insert(req.key, nx);
                            nx
                        }
                    };
                    self.g
                        .update_edge(req_nx, *feature_nx, Edge::Requires(req.validator));
                }
            } else {
                unreachable!()
            }
        }

        let topo = match petgraph::algo::toposort(&self.g, None) {
            Ok(topo) => topo,
            Err(node_in_cycle) => {
                tracing::error!("cycle detected: dot: {:#?}", self.to_dot());
                // there might be multiple cycles, we really only need to find
                // one though
                let mut cycle = vec![node_in_cycle.node_id()];
                let mut dfs = Dfs::new(&self.g, node_in_cycle.node_id());
                while let Some(nx) = dfs.next(&self.g) {
                    cycle.push(nx);
                    if self.g.neighbors(nx).contains(&node_in_cycle.node_id()) {
                        let mut cycle: Vec<_> = cycle
                            .into_iter()
                            // only include the features so that it doesn't
                            // overwhelm the user
                            .filter_map(|nx| match &self.g[nx] {
                                Node::PendingFeature(f) => Some(f.clone()),
                                _ => None,
                            })
                            .collect();
                        // Rotate the cycle so that the "minimum value" feature
                        // is first. This is semantically meaningless but does
                        // guarantee that cycle error messages are deterministic
                        let min_index = cycle
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| a.cmp(b))
                            .expect("there is always at least one element")
                            .0;
                        cycle.rotate_left(min_index);
                        return Err(Error::Cycle(Cycle(cycle)));
                    }
                }
                unreachable!()
            }
        };

        for nx in self.g.node_indices() {
            match &self.g[nx] {
                // If multiple nodes provide the same item, fail now
                Node::Item(item) => {
                    let features_that_provide: Vec<_> = self
                        .g
                        .neighbors_directed(nx, Direction::Incoming)
                        .filter_map(|nx| match &self.g[nx] {
                            Node::PendingFeature(f) | Node::ParentFeature(f) => Some(f),
                            _ => None,
                        })
                        .collect();
                    if features_that_provide.len() > 1 {
                        return Err(Error::Conflict {
                            item: item.clone(),
                            features: features_that_provide.into_iter().cloned().collect(),
                        });
                    }
                }
                _ => (),
            }
        }
        // If there are any items that exist but fail validation rules, return
        // an Err now
        for edge in self.g.edge_indices() {
            match self.g.edge_weight(edge).expect("definitely exists") {
                Edge::Requires(validator) => {
                    let (item, _feature) = self.g.edge_endpoints(edge).expect("definitely exists");
                    match &self.g[item] {
                        Node::Item(item) => {
                            if !validator.satisfies(item) {
                                return Err(Error::Unsatisfied {
                                    item: item.clone(),
                                    validator: validator.clone(),
                                });
                            }
                        }
                        Node::MissingItem(key) => {
                            if *validator != Validator::DoesNotExist {
                                return Err(Error::MissingItem { key: key.clone() });
                            }
                        }
                        _ => unreachable!("Requires edges cannot exist on anything but Items"),
                    }
                }
                _ => (),
            }
        }

        Ok(Graph {
            g: self.g,
            root: self.root,
            items: self.items,
            topo,
            end: self.phases[&Phase::End],
        })
    }
}

#[serde_as]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Graph<'a> {
    #[serde(borrow)]
    g: DiGraph<Node<'a>, Edge>,
    root: NodeIndex<DefaultIx>,
    #[serde_as(as = "Vec<(_, _)>")]
    items: HashMap<ItemKey<'a>, NodeIndex<DefaultIx>>,
    topo: Vec<NodeIndex<DefaultIx>>,
    end: (NodeIndex<DefaultIx>, NodeIndex<DefaultIx>),
}

impl<'a> Graph<'a> {
    pub fn builder(parent: Option<Self>) -> GraphBuilder<'a> {
        GraphBuilder::new(parent)
    }

    pub fn to_dot<'b>(&'b self) -> dot::Dot<'a, 'b> {
        dot::Dot(&self.g)
    }

    pub fn pending_features(&self) -> impl Iterator<Item = &Feature<'a>> {
        self.topo.iter().filter_map(|nx| match &self.g[*nx] {
            Node::PendingFeature(f) => Some(f),
            _ => None,
        })
    }

    /// There are many image features that produce items that we cannot
    /// reasonably know ahead-of-time (for example, rpm installation). This
    /// method will add [Item] nodes for anything newly discovered in the
    /// filesystem and add it to the end of the graph since we don't know where
    /// it came from.
    pub fn populate_dynamic_items(&mut self, root: &Path) -> std::io::Result<()> {
        for entry in walkdir::WalkDir::new(root) {
            let entry = entry?;
            let path =
                Path::new("/").join(entry.path().strip_prefix(root).expect("this must succeed"));
            let key = ItemKey::Path(path.clone().into());
            if let std::collections::hash_map::Entry::Vacant(e) = self.items.entry(key) {
                let meta = entry.metadata()?;
                let nx =
                    self.g
                        .add_node(Node::Item(Item::Path(item::Path::Entry(item::FsEntry {
                            path: path.into(),
                            mode: meta.mode(),
                            file_type: meta.file_type().into(),
                        }))));
                e.insert(nx);
                self.g.update_edge(self.end.0, nx, Edge::PartOf);
                self.g.update_edge(nx, self.end.1, Edge::After);
            }
        }
        let passwd_path = root.join("etc/passwd");
        let passwd = if passwd_path.exists() {
            antlir2_users::passwd::EtcPasswd::parse(&std::fs::read_to_string(passwd_path)?)
                .map_err(std::io::Error::other)?
                .into_owned()
        } else {
            Default::default()
        };
        for user in passwd.into_records() {
            if let std::collections::hash_map::Entry::Vacant(e) =
                self.items.entry(ItemKey::User(user.name.clone()))
            {
                let nx = self
                    .g
                    .add_node(Node::Item(Item::User(item::User { name: user.name })));
                e.insert(nx);
                self.g.update_edge(self.end.0, nx, Edge::PartOf);
                self.g.update_edge(nx, self.end.1, Edge::After);
            }
        }
        let group_path = root.join("etc/group");
        let groups = if group_path.exists() {
            antlir2_users::group::EtcGroup::parse(&std::fs::read_to_string(group_path)?)
                .map_err(std::io::Error::other)?
                .into_owned()
        } else {
            Default::default()
        };
        for group in groups.into_records() {
            if let std::collections::hash_map::Entry::Vacant(e) =
                self.items.entry(ItemKey::Group(group.name.clone()))
            {
                let nx = self
                    .g
                    .add_node(Node::Item(Item::Group(item::Group { name: group.name })));
                e.insert(nx);
                self.g.update_edge(self.end.0, nx, Edge::PartOf);
                self.g.update_edge(nx, self.end.1, Edge::After);
            }
        }
        Ok(())
    }
}
