// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Vote kicking of players in a running game (`OpenRA.Game/Server/VoteKickTracker.cs`).

use std::collections::HashMap;
use std::time::Instant;

use indexmap::IndexMap;
use rustora_net::order::RawOrder;
use rustora_net::session::Client;

use crate::server::{Server, ServerState};

/// The running vote: clients are referred to by their index, which also identifies their connection.
#[derive(Default)]
pub struct VoteKickTracker {
    votes: IndexMap<i32, bool>,
    failed_vote_kickers: HashMap<i32, u64>,
    timer: Option<Instant>,
    kickee: Option<(i32, String)>,
    starter: Option<i32>,
}

impl Server {
    fn client_has_power(&self, client: &Client) -> bool {
        client.is_admin || (!client.is_observer() && !self.has_client_won_or_lost(client.index))
    }

    fn kick_vote_order(&mut self, name: &str, kickee: i32) {
        let order = RawOrder {
            extra_data: kickee as u32,
            ..RawOrder::new(name)
        };
        self.dispatch_server_orders_to_clients(0, &order.serialize());
    }

    /// `VoteKickTracker.Tick`.
    pub fn vote_kick_tick(&mut self) {
        let Some(timer) = self.vote_kick.timer else {
            return;
        };
        let kickee_online = self.vote_kick.kickee.as_ref().is_some_and(|k| self.conn(k.0).is_some());
        if !kickee_online {
            self.end_kick_vote(true);
        } else if timer.elapsed().as_millis() as u64 > self.settings.vote_kick_timer {
            self.end_kick_vote_and_block_kicker();
        }
    }

    /// `VoteKickTracker.VoteKick`: true if the vote kicks `kickee`.
    pub fn vote_kick(&mut self, conn: i32, kicker: &Client, kickee: &Client, vote: bool) -> bool {
        let in_progress = self.vote_kick.timer.is_some();
        let other_vote_running = in_progress && self.vote_kick.kickee.as_ref().is_none_or(|k| k.0 != kickee.index);
        if self.state != ServerState::GameStarted || (!in_progress && !vote) || other_vote_running || !self.client_has_power(kicker) {
            self.send_fluent_message_to(conn, "notification-unable-to-start-a-vote", &[]);
            return false;
        }

        let kickee_name = kickee.name.clone().unwrap_or_default();
        let mut eligible = 0;
        let mut admin_dead_but_online = false;
        for c in &self.conns {
            let Some(client) = self.lobby.client_with_index(c.player_index) else {
                continue;
            };
            if client.index != kickee.index && self.client_has_power(client) {
                eligible += 1;
            }
            if client.is_admin && (client.is_observer() || self.has_client_won_or_lost(client.index)) {
                admin_dead_but_online = true;
            }
        }
        let Some(kickee_timer) = self.conn(kickee.index).map(|c| c.connection_timer()) else {
            self.end_kick_vote(true);
            return false;
        };

        if eligible < 2 || (admin_dead_but_online && !kickee.is_admin && eligible < 3) {
            if !kickee.is_observer() && !self.has_client_won_or_lost(kickee.index) {
                self.send_fluent_message_to(
                    conn,
                    "notification-insufficient-votes-to-kick",
                    &[("kickee", (&kickee_name).into())],
                );
                self.end_kick_vote(true);
                return false;
            } else if vote {
                self.end_kick_vote(false);
                return true;
            }
        }

        if !in_progress {
            if let Some(&time) = self.vote_kick.failed_vote_kickers.get(&kicker.index) {
                if time + self.settings.vote_kicker_cooldown > kickee_timer {
                    self.send_fluent_message_to(conn, "notification-unable-to-start-a-vote", &[]);
                    return false;
                }
                self.vote_kick.failed_vote_kickers.remove(&kicker.index);
            }
            self.vote_kick.timer = Some(Instant::now());
            let kicker_name = kicker.name.clone().unwrap_or_default();
            self.send_fluent_message(
                "notification-vote-kick-started",
                &[("kicker", (&kicker_name).into()), ("kickee", (&kickee_name).into())],
            );
            self.kick_vote_order("StartKickVote", kickee.index);
            self.vote_kick.kickee = Some((kickee.index, kickee_name.clone()));
            self.vote_kick.starter = Some(conn);
        }

        if self.vote_kick.votes.contains_key(&conn) {
            self.send_fluent_message_to(conn, "notification-kick-already-voted", &[]);
            return false;
        }
        self.vote_kick.votes.insert(conn, vote);

        let votes_for = self.vote_kick.votes.values().filter(|v| **v).count() as i64;
        let mut votes_against = self.vote_kick.votes.len() as i64 - votes_for;
        let mut eligible = eligible as i64;
        if self.client_has_power(kickee) {
            eligible += 1;
            votes_against += 1;
        }
        let votes_needed = eligible / 2 + 1;
        self.send_fluent_message(
            "notification-vote-kick-in-progress",
            &[
                ("kickee", (&kickee_name).into()),
                ("percentage", (votes_for * 100 / eligible).into()),
            ],
        );
        if vote && votes_for >= votes_needed {
            self.end_kick_vote(false);
            return true;
        }
        if eligible - votes_against < votes_needed {
            self.end_kick_vote_and_block_kicker();
            return false;
        }
        self.vote_kick.timer = Some(Instant::now());
        false
    }

    fn end_kick_vote_and_block_kicker(&mut self) {
        if self.vote_kick.timer.is_none() {
            return;
        }
        if let Some(starter) = self.vote_kick.starter
            && let Some(timer) = self.conn(starter).map(|c| c.connection_timer())
        {
            self.vote_kick.failed_vote_kickers.insert(starter, timer);
        }
        self.end_kick_vote(true);
    }

    fn end_kick_vote(&mut self, send_message: bool) {
        if self.vote_kick.timer.is_none() {
            return;
        }
        if let Some((index, name)) = self.vote_kick.kickee.take() {
            if send_message {
                self.send_fluent_message("notification-vote-kick-ended", &[("kickee", (&name).into())]);
            }
            self.kick_vote_order("EndKickVote", index);
        }
        self.vote_kick.timer = None;
        self.vote_kick.starter = None;
        self.vote_kick.votes.clear();
    }
}
