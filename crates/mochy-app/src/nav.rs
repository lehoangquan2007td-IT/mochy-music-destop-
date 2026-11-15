//! Navigation routes with back/forward history.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibTab {
    Liked,
    Playlists,
    History,
    MostPlayed,
    Local,
}

impl LibTab {
    pub const ALL: [LibTab; 5] = [LibTab::Liked, LibTab::Playlists, LibTab::History, LibTab::MostPlayed, LibTab::Local];
    pub fn label(self) -> &'static str {
        match self {
            LibTab::Liked => "Liked songs",
            LibTab::Playlists => "Playlists",
            LibTab::History => "History",
            LibTab::MostPlayed => "Most played",
            LibTab::Local => "Local files",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    Search,
    Library(LibTab),
    LocalPlaylist(String),
    Album(String),
    Artist(String),
    Playlist(String),
    AudioLab,
    Settings,
}

impl Route {
    /// Sidebar section the route belongs to.
    pub fn section(&self) -> u8 {
        match self {
            Route::Home | Route::Album(_) | Route::Artist(_) | Route::Playlist(_) => 0,
            Route::Search => 1,
            Route::Library(_) | Route::LocalPlaylist(_) => 2,
            Route::AudioLab => 3,
            Route::Settings => 4,
        }
    }
}

pub struct Nav {
    stack: Vec<Route>,
    pos: usize,
}

impl Default for Nav {
    fn default() -> Self {
        Self { stack: vec![Route::Home], pos: 0 }
    }
}

impl Nav {
    pub fn current(&self) -> &Route {
        &self.stack[self.pos]
    }

    pub fn go(&mut self, r: Route) {
        if *self.current() == r {
            return;
        }
        self.stack.truncate(self.pos + 1);
        self.stack.push(r);
        if self.stack.len() > 100 {
            self.stack.remove(0);
        }
        self.pos = self.stack.len() - 1;
    }

    /// Replace the current route without adding history (tab switches).
    pub fn replace(&mut self, r: Route) {
        self.stack[self.pos] = r;
    }

    pub fn can_back(&self) -> bool {
        self.pos > 0
    }
    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.stack.len()
    }
    pub fn back(&mut self) {
        if self.can_back() {
            self.pos -= 1;
        }
    }
    pub fn forward(&mut self) {
        if self.can_forward() {
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_forward_and_truncate() {
        let mut n = Nav::default();
        n.go(Route::Search);
        n.go(Route::Album("a".into()));
        n.back();
        assert_eq!(n.current(), &Route::Search);
        n.forward();
        assert_eq!(n.current(), &Route::Album("a".into()));
        n.back();
        n.go(Route::Settings);
        assert!(!n.can_forward());
        n.go(Route::Settings);
        n.back();
        assert_eq!(n.current(), &Route::Search);
    }
}
