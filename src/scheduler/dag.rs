use crate::config::Plan;
use anyhow::Result;
use petgraph::algo::is_cyclic_directed;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use std::collections::HashMap;

pub struct TaskGraph {
    graph: DiGraph<String, ()>,
    index: HashMap<String, NodeIndex>,
}

impl TaskGraph {
    pub fn from_plan(plan: &Plan) -> Result<Self> {
        let mut graph = DiGraph::<String, ()>::new();
        let mut index = HashMap::new();
        for t in &plan.tasks {
            let idx = graph.add_node(t.id.clone());
            index.insert(t.id.clone(), idx);
        }
        for t in &plan.tasks {
            let to = index[&t.id];
            for dep in &t.depends_on {
                let from = *index
                    .get(dep)
                    .ok_or_else(|| anyhow::anyhow!("unknown dep {dep}"))?;
                graph.add_edge(from, to, ());
            }
        }

        if is_cyclic_directed(&graph) {
            anyhow::bail!("PLAN.yaml has a dependency cycle");
        }

        Ok(Self { graph, index })
    }

    pub fn initial_ready(&self) -> Vec<String> {
        self.graph
            .node_indices()
            .filter(|n| {
                self.graph
                    .neighbors_directed(*n, Direction::Incoming)
                    .next()
                    .is_none()
            })
            .map(|n| self.graph[n].clone())
            .collect()
    }

    pub fn downstream(&self, id: &str) -> Vec<String> {
        let Some(idx) = self.index.get(id) else {
            return vec![];
        };
        self.graph
            .neighbors_directed(*idx, Direction::Outgoing)
            .map(|n| self.graph[n].clone())
            .collect()
    }

    pub fn upstream(&self, id: &str) -> Vec<String> {
        let Some(idx) = self.index.get(id) else {
            return vec![];
        };
        self.graph
            .neighbors_directed(*idx, Direction::Incoming)
            .map(|n| self.graph[n].clone())
            .collect()
    }
}
