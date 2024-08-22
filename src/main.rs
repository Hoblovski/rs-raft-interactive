#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_mut)]
use std::{cell::RefCell, cmp::min, io::{stdin, Write}, rc::Rc};

type NodeId = i32;
type Value = i32;

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord)]

struct LogEntry {
    term: i32,
    index: i32,
}

#[derive(Debug, PartialEq, Clone)]
struct LogEntryValue {
    ent: LogEntry,
    val: Value,
}

#[derive(Debug, PartialEq, Clone)]
enum MessageData {
    RequestVoteRequest {
        term: i32,
        cand_id: NodeId,
        last: LogEntry,
    },
    RequestVoteResponse {
        term: i32,
        granted: bool,
    },
    AppendEntriesRequest {
        term: i32,
        leader_id: i32,
        prev: LogEntry,
        entries: Vec<LogEntryValue>,
        leader_commit: i32,
    },
    AppendEntriesResponse {
        term: i32,
        success: bool,
        last: Option<LogEntry>,
    },
}

#[derive(Debug, PartialEq)]
struct Message {
    from: NodeId,
    to: NodeId,
    data: MessageData,
}

#[derive(Debug, PartialEq)]
enum NetworkError {
    E0,
    NoMessage,
}

#[derive(Debug)]
struct Network {
    msgs: RefCell<Vec<Message>>,
}

impl Network {
    fn new() -> Self {
        Self {
            msgs: RefCell::new(Vec::new()),
        }
    }

    fn send(&self, m: Message) -> Result<(), NetworkError> {
        let mut msgs = self.msgs.borrow_mut();
        msgs.push(m);
        Ok(())
    }

    fn recv(&self, id: NodeId) -> Result<Message, NetworkError> {
        let mut msgs = self.msgs.borrow_mut();
        let Some(idx) = msgs.iter().position(|x| x.to == id) else {
            return Err(NetworkError::NoMessage);
        };
        let msg = msgs.remove(idx);
        Ok(msg)
    }

    fn pull<F>(&self, f: F) -> Option<Message>
    where
        F: Fn(&Message) -> bool,
    {
        let mut msgs = self.msgs.borrow_mut();
        let Some(idx) = msgs.iter().position(f) else {
            return None;
        };
        let msg = msgs.remove(idx);
        Some(msg)
    }

    fn pull_ith(&self, idx: usize) -> Message {
        let mut msgs = self.msgs.borrow_mut();
        msgs.remove(idx)
    }

    fn peek_idx<F>(&self, f: F) -> Option<usize>
    where
        F: Fn(&Message) -> bool,
    {
        let mut msgs = self.msgs.borrow_mut();
        msgs.iter().position(f)
    }

    fn lose_package(&self) {
        let mut msgs = self.msgs.borrow_mut();
        if msgs.is_empty() {
            return;
        }
        let idx = rand::random::<usize>() % msgs.len();
        msgs.remove(idx);
    }
}

#[derive(Debug, PartialEq)]
enum NodeState {
    Follower,
    Candidate,
    Leader,
}

impl NodeState {
    fn one_letter(&self) -> &'static str {
        match self {
            NodeState::Follower => "F",
            NodeState::Candidate => "C",
            NodeState::Leader => "L",
        }
    }
}

#[derive(Debug)]
struct RaftNode {
    // persistent values
    current_term: i32,
    voted_for: Option<NodeId>,
    log: Vec<LogEntryValue>,

    // volatile values
    commit_index: i32,
    last_applied: i32,

    // leader state
    next_index: Vec<i32>,
    match_index: Vec<i32>,

    // for bookkeeping
    network_size: usize,
    network: Rc<Network>,
    id: i32,
    state: NodeState,

    // voting
    votes_received: Vec<i32>,
}

impl RaftNode {
    fn restart(&mut self) {
        self.commit_index = 0;
        self.last_applied = 0;
        self.next_index.fill(-1);
        self.match_index.fill(-1);
        self.state = NodeState::Follower;
        self.votes_received.clear();
        // keep: current_term, voted_for, log
    }

    fn new_follower(id: i32, network: Rc<Network>, network_size: usize) -> Self {
        let init_entry = LogEntryValue {
            ent: LogEntry { term: 0, index: 0 },
            val: 0,
        };
        RaftNode {
            current_term: 0,
            voted_for: None,
            log: vec![init_entry],
            commit_index: 0,
            last_applied: 0,
            next_index: vec![-1; network_size],
            match_index: vec![-1; network_size],
            network_size,
            id,
            network,
            state: NodeState::Follower,
            votes_received: vec![],
        }
    }

    fn new(network_size: usize, network: Rc<Network>) -> Vec<Self> {
        return (0..network_size)
            .map(|id| Self::new_follower(id as i32, network.clone(), network_size))
            .collect();
    }

    //-----------------
    // helpers
    fn well_formed(&self) -> bool {
        let index_ok = self
            .log
            .iter()
            .enumerate()
            .all(|(i, ev)| i as i32 == ev.ent.index);
        index_ok
    }

    fn reply_request_vote(&self, dest: i32, granted: bool) {
        let data = MessageData::RequestVoteResponse {
            term: self.current_term,
            granted,
        };
        let msg = Message {
            from: self.id,
            to: dest,
            data,
        };
        self.network.send(msg).unwrap();
    }

    fn reply_append_entries(&self, dest: i32, success: bool, last: Option<LogEntry>) {
        let data = MessageData::AppendEntriesResponse {
            term: self.current_term,
            success,
            last,
        };
        let msg = Message {
            from: self.id,
            to: dest,
            data,
        };
        self.network.send(msg).unwrap();
    }

    fn convert_to_follower(&mut self, new_term: i32) {
        assert!(new_term > self.current_term);
        self.current_term = new_term;
        self.state = NodeState::Follower;
        self.voted_for = None;
        self.votes_received = vec![]; // not necessary
    }

    fn become_leader(&mut self) {
        self.state = NodeState::Leader;
        self.next_index.fill(self.log.len() as i32);
        self.match_index.fill(0);
        // after self becomes leader, append entries are enabled
        // but not executed right away
        // self.append_entries_all();
    }

    fn maybe_advance_commit_index(&mut self) {
        let mut match_indices = self.match_index.clone();
        match_indices[self.id as usize] = self.log.len() as i32;
        match_indices.sort();
        // the maximum value such that a majority of match_index values are geq than it
        self.commit_index = match_indices[(match_indices.len() - 1) / 2];
    }
}

// TODO: make steps smaller.
impl RaftNode {
    //-----------------------------
    // STEP timeout
    fn heartbeat_timeout_cond(&mut self) -> bool {
        self.state == NodeState::Follower || self.state == NodeState::Candidate
    }

    fn heartbeat_timeout(&mut self) {
        assert!(self.heartbeat_timeout_cond());
        self.state = NodeState::Candidate;
        self.current_term += 1;
        self.voted_for = Some(self.id);
        self.votes_received = vec![self.id];
        // do not send request vote requests right away
        // they are in a seperate step
        // self.request_vote_all_others();
    }

    //-----------------------------
    // STEP client_request
    fn client_request_cond(&mut self) -> bool {
        // Ignore: redirection.
        return self.state == NodeState::Leader;
    }

    // Ignore: response to client
    fn client_request(&mut self, val: Value) {
        assert!(self.client_request_cond());
        self.log.push(LogEntryValue {
            ent: LogEntry {
                term: self.current_term,
                index: self.log.len() as i32,
            },
            val,
        });
    }

    //-----------------------------
    // STEP send_request_vote
    fn send_request_vote_cond(&mut self, dest: i32) -> bool {
        return self.state == NodeState::Candidate && self.id != dest;
    }

    fn send_request_vote(&mut self, dest: i32) {
        assert!(self.send_request_vote_cond(dest));

        let data = MessageData::RequestVoteRequest {
            term: self.current_term,
            cand_id: self.id,
            last: self.log.last().unwrap().ent.clone(),
        };
        let msg = Message {
            from: self.id,
            to: dest as i32,
            data,
        };
        self.network.send(msg).unwrap();
    }

    //-----------------------------
    // STEP send_append_entries
    fn send_append_entries_cond(&mut self, dest: i32) -> bool {
        return self.state == NodeState::Leader && self.id != dest;
    }

    fn send_append_entries(&mut self, dest: i32) {
        assert!(self.send_append_entries_cond(dest));
        let ni = self.next_index[dest as usize] as usize;
        let data = MessageData::AppendEntriesRequest {
            term: self.current_term,
            leader_id: self.id,
            prev: self.log[ni - 1].ent.clone(),
            entries: self.log[ni..].to_vec(),
            leader_commit: self.commit_index,
        };
        let msg = Message {
            from: self.id,
            to: dest as i32,
            data,
        };
        self.network.send(msg).unwrap();
    }

    //-----------------------------
    // STEP recv_RequestVoteRequest
    fn recv_request_vote_request_cond(&mut self, m: &Message) -> bool {
        let Message { from, to, data } = m;
        if to != &self.id {
            return false;
        }
        return matches!(data, MessageData::RequestVoteRequest { .. });
    }

    fn recv_request_vote_request(&mut self, m: Message) {
        assert!(self.recv_request_vote_request_cond(&m));
        let Message { from, to, data } = m;
        let MessageData::RequestVoteRequest {
            term,
            cand_id,
            last,
        } = data
        else {
            unreachable!()
        };

        if term < self.current_term {
            self.reply_request_vote(from, false);
            return;
        }
        if term > self.current_term {
            self.convert_to_follower(term);
            return;
        }

        let my_last = &self.log.last().unwrap().ent;
        let log_ok = &last >= my_last;
        let votefor_ok = !self.voted_for.is_some_and(|x| x != from);
        let granted = votefor_ok && log_ok;

        if granted {
            self.voted_for = Some(from);
        }
        self.reply_request_vote(from, granted);
    }

    //-----------------------------
    // STEP recv_RequestVoteResponse
    fn recv_request_vote_response_cond(&mut self, m: &Message) -> bool {
        let Message { from, to, data } = m;
        if to != &self.id {
            return false;
        }
        return matches!(data, MessageData::RequestVoteResponse { .. });
    }

    fn recv_request_vote_response(&mut self, m: Message) {
        assert!(self.recv_request_vote_response_cond(&m));
        let Message { from, to, data } = m;
        let MessageData::RequestVoteResponse { term, granted } = data else {
            unreachable!()
        };

        if term < self.current_term {
            return;
        }
        if term > self.current_term {
            self.convert_to_follower(term);
            return;
        }

        if granted {
            if !self.votes_received.contains(&from) {
                self.votes_received.push(from);
            }
        }

        if self.votes_received.len() > self.network_size / 2 {
            self.become_leader();
        }
    }

    //-----------------------------
    // STEP recv_AppendEntriesRequest
    fn recv_append_entries_request_cond(&mut self, m: &Message) -> bool {
        let Message { from, to, data } = m;
        if to != &self.id {
            return false;
        }
        return matches!(data, MessageData::AppendEntriesRequest { .. });
    }

    fn recv_append_entries_request(&mut self, m: Message) {
        assert!(self.recv_append_entries_request_cond(&m));

        let Message { from, to, data } = m;
        let MessageData::AppendEntriesRequest {
            term,
            leader_id,
            prev,
            entries,
            leader_commit,
        } = data
        else {
            unreachable!()
        };

        if term < self.current_term {
            self.reply_append_entries(from, false, None);
            return;
        }
        if term > self.current_term {
            self.convert_to_follower(term);
            return;
        }

        if (prev.index as usize) >= self.log.len() || self.log[prev.index as usize].ent.term != prev.term {
            self.reply_append_entries(from, false, None);
            return;
        }

        // update log entries and commit index
        let lastnewi = prev.index + entries.len() as i32;
        self.log.truncate(prev.index as usize + 1);
        self.log.extend(entries);
        if leader_commit > self.commit_index {
            self.commit_index = min(leader_commit, lastnewi);
        }
        let last = self.log.last().unwrap().ent.clone();
        self.reply_append_entries(from, true, Some(last));
    }

    //-----------------------------
    // STEP recv_AppendEntriesResponse
    fn recv_append_entries_response_cond(&mut self, m: &Message) -> bool {
        let Message { from, to, data } = m;
        if to != &self.id {
            return false;
        }
        return matches!(data, MessageData::AppendEntriesResponse { .. });
    }

    fn recv_append_entries_response(&mut self, m: Message) {
        assert!(self.recv_append_entries_response_cond(&m));
        let Message { from, to, data } = m;
        let MessageData::AppendEntriesResponse {
            term,
            success,
            last,
        } = data
        else {
            unreachable!()
        };

        if term < self.current_term {
            return;
        }
        if term > self.current_term {
            self.convert_to_follower(term);
            return;
        }

        if success {
            // UPDATE matchindex and nextindex
            let last_index = last.unwrap().index;
            self.next_index[from as usize] = last_index + 1;
            self.match_index[from as usize] = last_index;
            self.maybe_advance_commit_index();
        } else {
            self.next_index[from as usize] -= 1;
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WorldStep {
    // misc
    // crashed nodes cannot initiate nodewise actions and cannot receive messages
    Crash { id: i32 },
    Restart { id: i32 },
    // spontaneous actions:
    HeartBeatTimeout { id: i32 },
    ClientRequest { id: i32, value: Value },
    SendAppendEntries { id: i32, dest: i32 },
    SendRequestVote { id: i32, dest: i32 },
    // receive actions:
    RecvRequestVoteRequest { id: i32, msg_idx: usize },
    RecvRequestVoteResponse { id: i32, msg_idx: usize },
    RecvAppendEntriesRequest { id: i32, msg_idx: usize },
    RecvAppendEntriesResponse { id: i32, msg_idx: usize },
}
struct World {
    network: Rc<Network>,
    nodes: Vec<RaftNode>,
    crashed: Vec<i32>,
    n_steps: usize,
    n_nodes: usize,
}

impl World {
    fn parse_choice(&self, s: &str) -> Option<WorldStep> {
        let mut parts = s.split_ascii_whitespace();
        let leader = parts.next().unwrap();
        match leader {
            "crash" => {
                let id = parts.next().unwrap().parse::<i32>().unwrap();
                Some(WorldStep::Crash { id })
            }
            "restart" => {
                let id = parts.next().unwrap().parse::<i32>().unwrap();
                Some(WorldStep::Restart { id })
            }
            "timeout" => {
                let id = parts.next().unwrap().parse::<i32>().unwrap();
                Some(WorldStep::HeartBeatTimeout { id })
            }
            "client" => {
                let id = parts.next().unwrap().parse::<i32>().unwrap();
                let value = self.n_steps as i32;
                Some(WorldStep::ClientRequest { id, value })
            }
            "ae" | "appendentries" => {
                let id = parts.next().unwrap().parse::<i32>().unwrap();
                let dest = parts.next().unwrap().parse::<i32>().unwrap();
                Some(WorldStep::SendAppendEntries { id, dest })
            }
            "rv" | "requestvote" => {
                let id = parts.next().unwrap().parse::<i32>().unwrap();
                let dest = parts.next().unwrap().parse::<i32>().unwrap();
                Some(WorldStep::SendRequestVote { id, dest })
            }
            "recv" => {
                let msg_idx = parts.next().unwrap().parse::<i32>().unwrap();
                let msg = &self.network.msgs.borrow()[msg_idx as usize];
                let id = msg.to;
                Some(match msg.data {
                    MessageData::RequestVoteRequest { .. } => WorldStep::RecvRequestVoteRequest {
                        id,
                        msg_idx: msg_idx as usize,
                    },
                    MessageData::RequestVoteResponse { .. } => WorldStep::RecvRequestVoteResponse {
                        id,
                        msg_idx: msg_idx as usize,
                    },
                    MessageData::AppendEntriesRequest { .. } => {
                        WorldStep::RecvAppendEntriesRequest {
                            id,
                            msg_idx: msg_idx as usize,
                        }
                    }
                    MessageData::AppendEntriesResponse { .. } => {
                        WorldStep::RecvAppendEntriesResponse {
                            id,
                            msg_idx: msg_idx as usize,
                        }
                    }
                })
            }
            _ => None,
        }
    }

    fn new(n_nodes: usize) -> Self {
        let network = Rc::new(Network::new());
        let nodes = RaftNode::new(n_nodes, network.clone());
        Self {
            network,
            nodes,
            crashed: vec![],
            n_nodes,
            n_steps: 0,
        }
    }

    fn candidate_steps(&mut self) -> Vec<WorldStep> {
        let mut cand_steps: Vec<WorldStep> = vec![];
        // node-wise actions
        for id in 0..self.n_nodes {
            if self.crashed.contains(&(id as i32)) {
                cand_steps.push(WorldStep::Restart { id: id as i32 });
                continue;
            }
            cand_steps.push(WorldStep::Crash { id: id as i32 });
            let node = &mut self.nodes[id];
            if node.heartbeat_timeout_cond() {
                cand_steps.push(WorldStep::HeartBeatTimeout { id: id as i32 });
            }
            if node.client_request_cond() {
                cand_steps.push(WorldStep::ClientRequest {
                    id: id as i32,
                    value: self.n_steps as i32,
                });
            }
            for dest in 0..self.n_nodes {
                if node.send_append_entries_cond(dest as i32) {
                    cand_steps.push(WorldStep::SendAppendEntries {
                        id: id as i32,
                        dest: dest as i32,
                    })
                }
                if node.send_request_vote_cond(dest as i32) {
                    cand_steps.push(WorldStep::SendRequestVote {
                        id: id as i32,
                        dest: dest as i32,
                    })
                }
            }
        }
        // message-wise actions
        for (msg_idx, msg) in self.network.msgs.borrow().iter().enumerate() {
            if self.crashed.contains(&msg.to) {
                continue;
            }
            match msg.data {
                MessageData::RequestVoteRequest { .. } => {
                    cand_steps.push(WorldStep::RecvRequestVoteRequest {
                        id: msg.to,
                        msg_idx,
                    })
                }
                MessageData::RequestVoteResponse { .. } => {
                    cand_steps.push(WorldStep::RecvRequestVoteResponse {
                        id: msg.to,
                        msg_idx,
                    })
                }
                MessageData::AppendEntriesRequest { .. } => {
                    cand_steps.push(WorldStep::RecvAppendEntriesRequest {
                        id: msg.to,
                        msg_idx,
                    })
                }
                MessageData::AppendEntriesResponse { .. } => {
                    cand_steps.push(WorldStep::RecvAppendEntriesResponse {
                        id: msg.to,
                        msg_idx,
                    })
                }
            }
        }
        cand_steps.sort();
        cand_steps
    }

    fn interactive_dump(&self) {
        println!("[dump nodes] ==========");
        for node in &self.nodes {
            println!(
                "{} :: {} at {}",
                node.id,
                if self.crashed.contains(&node.id) { "X" } else { node.state.one_letter() },
                node.current_term
            );
            println!(
                "    voted_for: {:?}, votes_received: {:?}",
                node.voted_for, node.votes_received
            );
            print!("    logs:[");
            for (idx, entval) in node.log.iter().enumerate() {
                let ent = &entval.ent;
                assert!(ent.index == idx as i32);
                if idx as i32 == node.commit_index {
                    print!(" {:>2}C", ent.term);
                } else {
                    print!(" {:>3}", ent.term);
                }
            }
            println!("]");
        }
        println!("[dump network] ==========");
        let msgs = self.network.msgs.borrow();
        for msg in msgs.iter() {
            println!("{:?}", msg);
        }
        println!("[end of dump] ==========");
    }

    fn interactive_step(&mut self) {
        self.n_steps += 1;
        println!("\n[next step is {}] ==========", self.n_steps);
        self.interactive_dump();
        let cand_steps = self.candidate_steps();
        for (idx, cand_step) in cand_steps.iter().enumerate() {
            println!("[CHOICE {}]: {:?}", idx, cand_step);
        }

        let choice = loop {
            print!("ENTER CHOICE >> ");
            std::io::stdout().flush().unwrap();
            let mut buf = String::new();
            stdin()
                .read_line(&mut buf)
                .expect("interactive input failure");
            if let Ok(choice) = buf.trim().parse::<usize>() {
                if choice < cand_steps.len() {
                    break choice;
                }
            }
            let step = self.parse_choice(&buf).unwrap();
            if let Some(choice) = cand_steps.iter().position(|x| x == &step) {
                break choice;
            }
        };

        self.execute_step(&cand_steps[choice]);
    }

    fn execute_step(&mut self, choice: &WorldStep) {
        match *choice {
            WorldStep::Crash { id } => {
                self.crashed.push(id);
            }
            WorldStep::Restart { id } => {
                let idx = self.crashed.iter().position(|x| x == &id).unwrap();
                let id = self.crashed.remove(idx);
                self.nodes[id as usize].restart();
            }
            WorldStep::HeartBeatTimeout { id } => {
                let node = &mut self.nodes[id as usize];
                assert!(node.heartbeat_timeout_cond());
                node.heartbeat_timeout();
            }
            WorldStep::ClientRequest { id, value } => {
                let node = &mut self.nodes[id as usize];
                assert!(node.client_request_cond());
                node.client_request(value);
            }
            WorldStep::SendRequestVote { id, dest } => {
                let node = &mut self.nodes[id as usize];
                assert!(node.send_request_vote_cond(dest));
                node.send_request_vote(dest);
            }
            WorldStep::SendAppendEntries { id, dest } => {
                let node = &mut self.nodes[id as usize];
                assert!(node.send_append_entries_cond(dest));
                node.send_append_entries(dest);
            }
            WorldStep::RecvRequestVoteRequest { id, msg_idx } => {
                let node = &mut self.nodes[id as usize];
                let msg = self.network.pull_ith(msg_idx);
                assert!(node.recv_request_vote_request_cond(&msg));
                node.recv_request_vote_request(msg);
            }
            WorldStep::RecvRequestVoteResponse { id, msg_idx } => {
                let node = &mut self.nodes[id as usize];
                let msg = self.network.pull_ith(msg_idx);
                assert!(node.recv_request_vote_response_cond(&msg));
                node.recv_request_vote_response(msg);
            }
            WorldStep::RecvAppendEntriesRequest { id, msg_idx } => {
                let node = &mut self.nodes[id as usize];
                let msg = self.network.pull_ith(msg_idx);
                assert!(node.recv_append_entries_request_cond(&msg));
                node.recv_append_entries_request(msg);
            }
            WorldStep::RecvAppendEntriesResponse { id, msg_idx } => {
                let node = &mut self.nodes[id as usize];
                let msg = self.network.pull_ith(msg_idx);
                assert!(node.recv_append_entries_response_cond(&msg));
                node.recv_append_entries_response(msg);
            }
        }
    }
}

fn main() {
    let mut world = World::new(5);
    loop {
        world.interactive_step();
        println!("==========\n\n\n");
    }
}
