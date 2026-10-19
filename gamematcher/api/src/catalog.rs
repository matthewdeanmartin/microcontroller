//! The shared game catalog and who's into what (`spec/03-domain.md#games`).
//! Anyone may add a game; the person who added it, or an admin, may edit
//! it. Titles are unique after normalization, so "Catan" and "catan " are
//! the same game.
use crate::messages::{GameInput, StoredGame};
use crate::state::State;
use miniframework::ApiError;

pub const KINDS: [&str; 5] = ["board", "card", "video", "rpg", "other"];

/// Lower-case letters and digits only: what makes two titles the same game.
pub fn normalize(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// An error for a duplicate title; the API also sends the existing id.
pub fn duplicate(existing: &StoredGame) -> ApiError {
    ApiError::new(
        409,
        "duplicate_title",
        format!(
            "\u{201c}{}\u{201d} is already in the catalog",
            existing.title
        ),
    )
}

fn clean_title(raw: &str) -> Result<String, ApiError> {
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = title.chars().count();
    if (1..=48).contains(&len) && !normalize(&title).is_empty() {
        Ok(title)
    } else {
        Err(ApiError::bad_request("Title: 1 to 48 characters"))
    }
}

fn apply(game: &mut StoredGame, input: &GameInput) -> Result<(), ApiError> {
    if let Some(title) = &input.title {
        game.title = clean_title(title)?;
    }
    if let Some(kind) = &input.kind {
        if !KINDS.contains(&kind.as_str()) {
            return Err(ApiError::bad_request(
                "kind: board, card, video, rpg or other",
            ));
        }
        game.kind = kind.clone();
    }
    if let Some(n) = input.min_players {
        game.min_players = n;
    }
    if let Some(n) = input.max_players {
        game.max_players = n;
    }
    if game.min_players > 99 || game.max_players > 99 {
        return Err(ApiError::bad_request("Players: up to 99"));
    }
    if game.min_players > 0 && game.max_players > 0 && game.min_players > game.max_players {
        return Err(ApiError::bad_request(
            "Fewest players is more than most players",
        ));
    }
    if let Some(minutes) = input.minutes {
        if minutes > 24 * 60 {
            return Err(ApiError::bad_request("Length: up to 24 hours"));
        }
        game.minutes = minutes;
    }
    if let Some(notes) = &input.notes {
        let notes = notes.trim();
        if notes.chars().count() > 280 {
            return Err(ApiError::bad_request("Notes: up to 280 characters"));
        }
        game.notes = notes.to_string();
    }
    if let Some(archived) = input.archived {
        game.archived = archived;
    }
    Ok(())
}

impl State {
    pub fn game(&self, id: u32) -> Option<&StoredGame> {
        self.games.iter().find(|g| g.id == id)
    }

    fn game_index(&self, id: u32) -> Result<usize, ApiError> {
        self.games
            .iter()
            .position(|g| g.id == id)
            .ok_or_else(|| ApiError::not_found("No such game"))
    }

    /// The game whose normalized title equals `title`'s, other than `except`.
    pub fn game_titled(&self, title: &str, except: u32) -> Option<&StoredGame> {
        let key = normalize(title);
        self.games
            .iter()
            .find(|g| g.id != except && normalize(&g.title) == key)
    }

    pub fn can_edit_game(&self, actor: u32, game: &StoredGame) -> bool {
        game.added_by == actor || self.account(actor).is_some_and(|a| a.admin)
    }

    pub fn add_game(&mut self, actor: u32, input: &GameInput, now: u64) -> Result<u32, ApiError> {
        let title = input
            .title
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("A game needs a title"))?;
        if let Some(existing) = self.game_titled(title, 0) {
            return Err(duplicate(existing));
        }
        if self.games.len() >= self.limits.games {
            return Err(ApiError::new(
                409,
                "full",
                format!("The catalog holds at most {} games", self.limits.games),
            ));
        }
        let mut game = StoredGame {
            id: self.next_game,
            kind: "board".into(),
            added_by: actor,
            created: now,
            ..Default::default()
        };
        apply(
            &mut game,
            &GameInput {
                archived: None,
                ..input.clone()
            },
        )?;
        self.next_game += 1;
        let id = game.id;
        self.games.push(game);
        Ok(id)
    }

    pub fn edit_game(&mut self, actor: u32, id: u32, input: &GameInput) -> Result<(), ApiError> {
        let i = self.game_index(id)?;
        if !self.can_edit_game(actor, &self.games[i]) {
            return Err(ApiError::new(
                403,
                "not_yours",
                "Only whoever added a game, or an admin, can change it",
            ));
        }
        if let Some(title) = &input.title {
            if let Some(existing) = self.game_titled(title, id) {
                return Err(duplicate(existing));
            }
        }
        let mut game = self.games[i].clone();
        apply(&mut game, input)?;
        self.games[i] = game;
        Ok(())
    }

    pub fn set_interest(&mut self, account: u32, game: u32, on: bool) -> Result<(), ApiError> {
        if self.game(game).is_none() {
            return Err(ApiError::not_found("No such game"));
        }
        let a = self.account_mut(account)?;
        a.interests.retain(|&g| g != game);
        if on {
            a.interests.push(game);
        }
        Ok(())
    }

    pub fn interested_in(
        &self,
        game: u32,
    ) -> impl Iterator<Item = &crate::messages::StoredAccount> {
        self.accounts
            .iter()
            .filter(move |a| !a.disabled && a.interests.contains(&game))
    }

    fn game_in_plans(&self, id: u32) -> bool {
        self.plans.iter().any(|p| p.games.contains(&id))
    }

    /// Admin delete: a game some plan uses is archived instead.
    /// Returns true if it was really deleted.
    pub fn delete_game(&mut self, id: u32) -> Result<bool, ApiError> {
        let i = self.game_index(id)?;
        if self.game_in_plans(id) {
            self.games[i].archived = true;
            return Ok(false);
        }
        self.games.remove(i);
        for a in &mut self.accounts {
            a.interests.retain(|&g| g != id);
        }
        Ok(true)
    }

    /// Admin merge of a duplicate: interests, plans and ticks move to
    /// `into`, and `from` disappears.
    pub fn merge_games(&mut self, from: u32, into: u32) -> Result<(), ApiError> {
        if from == into {
            return Err(ApiError::bad_request("Pick a different game to merge into"));
        }
        let i = self.game_index(from)?;
        self.game_index(into)?;
        let swap = |list: &mut Vec<u32>| {
            if list.contains(&from) {
                list.retain(|&g| g != from);
                if !list.contains(&into) {
                    list.push(into);
                }
            }
        };
        for a in &mut self.accounts {
            swap(&mut a.interests);
        }
        for p in &mut self.plans {
            swap(&mut p.games);
            swap(&mut p.chosen_games);
            for r in &mut p.responses {
                swap(&mut r.games);
            }
        }
        self.games.remove(i);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Limits;

    fn state() -> (State, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            games: 3,
            ..Limits::board()
        });
        let admin = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let kid = s.signup("kid@h", "Sam", "1234", 0).unwrap();
        (s, admin, kid)
    }

    fn titled(title: &str) -> GameInput {
        GameInput {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    #[test]
    fn normalized_titles_collide() {
        assert_eq!(normalize(" Ticket to Ride! "), normalize("tickettoride"));
        let (mut s, _, kid) = state();
        let catan = s.add_game(kid, &titled("Catan"), 1).unwrap();
        let e = s.add_game(kid, &titled("  catan "), 1).unwrap_err();
        assert_eq!(e.code, "duplicate_title");
        assert_eq!(s.game_titled("CATAN", 0).unwrap().id, catan);
        assert_eq!(s.add_game(kid, &titled("!!!"), 1).unwrap_err().status, 400);
    }

    #[test]
    fn validation_and_bounds() {
        let (mut s, _, kid) = state();
        let bad = GameInput {
            min_players: Some(5),
            max_players: Some(2),
            ..titled("Wingspan")
        };
        assert_eq!(s.add_game(kid, &bad, 1).unwrap_err().status, 400);
        let bad_kind = GameInput {
            kind: Some("sport".into()),
            ..titled("Wingspan")
        };
        assert_eq!(s.add_game(kid, &bad_kind, 1).unwrap_err().status, 400);
        for t in ["A", "B", "C"] {
            s.add_game(kid, &titled(t), 1).unwrap();
        }
        assert_eq!(s.add_game(kid, &titled("D"), 1).unwrap_err().code, "full");
    }

    #[test]
    fn only_adder_or_admin_edits() {
        let (mut s, admin, kid) = state();
        let mom = s.signup("mom@h", "Mom", "1234", 0).unwrap();
        let g = s.add_game(kid, &titled("Catan"), 1).unwrap();
        let rename = titled("Settlers of Catan");
        assert_eq!(s.edit_game(mom, g, &rename).unwrap_err().code, "not_yours");
        s.edit_game(kid, g, &rename).unwrap();
        s.edit_game(admin, g, &titled("Catan")).unwrap();
        s.add_game(kid, &titled("Wingspan"), 1).unwrap();
        assert_eq!(
            s.edit_game(kid, g, &titled("wingspan")).unwrap_err().code,
            "duplicate_title"
        );
    }

    #[test]
    fn interests_merge_and_delete() {
        let (mut s, admin, kid) = state();
        let a = s.add_game(kid, &titled("Catan"), 1).unwrap();
        let b = s.add_game(kid, &titled("Settlers"), 1).unwrap();
        s.set_interest(kid, b, true).unwrap();
        s.set_interest(admin, a, true).unwrap();
        s.set_interest(admin, b, true).unwrap();
        assert_eq!(s.interested_in(b).count(), 2);
        s.merge_games(b, a).unwrap();
        assert!(s.game(b).is_none());
        assert_eq!(s.interested_in(a).count(), 2);
        assert_eq!(s.account(admin).unwrap().interests, vec![a], "no duplicate");
        assert!(s.delete_game(a).unwrap());
        assert!(s.account(kid).unwrap().interests.is_empty());
    }
}
