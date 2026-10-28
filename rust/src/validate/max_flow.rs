use std::collections::VecDeque;

const EPS: f64 = 1e-12;

/// Dinic's max flow on an adjacency list with real capacities. Edges are added and searched in the same order
/// as the Kotlin, so ties between equal flows break the same way.
pub struct MaxFlow {
    n: usize,
    head: Vec<i32>,
    to: Vec<usize>,
    next: Vec<i32>,
    cap: Vec<f64>,
    level: Vec<i32>,
    iter: Vec<i32>,
}

impl MaxFlow {
    pub fn new(n: usize) -> Self {
        Self { n, head: vec![-1; n], to: Vec::new(), next: Vec::new(), cap: Vec::new(), level: vec![0; n], iter: vec![0; n] }
    }

    /// Adds an edge u -> v and returns its id (for `flow_on`).
    pub fn edge(&mut self, u: usize, v: usize, c: f64) -> usize {
        let id = self.to.len();
        self.to.push(v);
        self.next.push(self.head[u]);
        self.cap.push(c);
        self.head[u] = id as i32;
        self.to.push(u);
        self.next.push(self.head[v]);
        self.cap.push(0.0);
        self.head[v] = id as i32 + 1;
        id
    }

    /// Flow pushed through edge `id` after `run`.
    pub fn flow_on(&self, id: usize) -> f64 {
        self.cap[id ^ 1]
    }

    pub fn run(&mut self, s: usize, t: usize) -> f64 {
        let mut total = 0.0;
        while self.bfs(s, t) {
            self.iter.copy_from_slice(&self.head);
            loop {
                let f = self.dfs(s, t, f64::MAX);
                if f <= EPS {
                    break;
                }
                total += f;
            }
        }
        total
    }

    fn bfs(&mut self, s: usize, t: usize) -> bool {
        self.level.fill(-1);
        self.level[s] = 0;
        let mut q = VecDeque::from([s]);
        while let Some(u) = q.pop_front() {
            let mut e = self.head[u];
            while e >= 0 {
                let ei = e as usize;
                if self.cap[ei] > EPS && self.level[self.to[ei]] < 0 {
                    self.level[self.to[ei]] = self.level[u] + 1;
                    q.push_back(self.to[ei]);
                }
                e = self.next[ei];
            }
        }
        self.level[t] >= 0
    }

    fn dfs(&mut self, u: usize, t: usize, f: f64) -> f64 {
        if u == t {
            return f;
        }
        while self.iter[u] >= 0 {
            let e = self.iter[u] as usize;
            let v = self.to[e];
            if self.cap[e] > EPS && self.level[v] == self.level[u] + 1 {
                let d = self.dfs(v, t, f.min(self.cap[e]));
                if d > EPS {
                    self.cap[e] -= d;
                    self.cap[e ^ 1] += d;
                    return d;
                }
            }
            self.iter[u] = self.next[e];
        }
        0.0
    }

    pub fn node_count(&self) -> usize {
        self.n
    }
}
