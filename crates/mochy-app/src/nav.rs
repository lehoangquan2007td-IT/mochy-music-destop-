//! Navigation routes with back/forward history.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibTab {
    Liked,
    Playlists,
    History,
    MostPlayed,
    Local,
    // ---- library/explore/stats ----
    Songs,
    Albums,
    Artists,
    RecentlyAdded,
    /// Downloaded songs + download queue (downloads module).
    Downloaded,
}

impl LibTab {
    pub const ALL: [LibTab; 10] = [
        LibTab::Songs,
        LibTab::Liked,
        LibTab::Albums,
        LibTab::Artists,
        LibTab::Playlists,
        LibTab::RecentlyAdded,
        LibTab::History,
        LibTab::MostPlayed,
        LibTab::Local,
        LibTab::Downloaded,
    ];
    pub fn label(self) -> &'static str {
        crate::ui::i18n::tr(match self {
            LibTab::Liked => "Liked songs",
            LibTab::Playlists => "Playlists",
            LibTab::History => "History",
            LibTab::MostPlayed => "Most played",
            LibTab::Local => "Local files",
            LibTab::Downloaded => "Downloaded",
            LibTab::Songs => "Songs",
            LibTab::Albums => "Albums",
            LibTab::Artists => "Artists",
            LibTab::RecentlyAdded => "Recently added",
        })
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
    // ---- library/explore/stats ----
    Explore,
    MoodsGenres,
    Charts,
    NewReleases,
    /// Generic YouTube Music browse page (genre / mood page, "see all").
    Browse { id: String, params: Option<String> },
    /// "See all" of an artist page shelf.
    ArtistMore { artist: String, title: String },
    /// Full list of a home shelf (`"quick_picks"`, `"recent"`, `"yt:<index>"`…).
    HomeSection(String),
    /// Artist / album page built from the local library (DB).
    LibraryArtist(String),
    LibraryAlbum(String),
    Stats,
    // ---- taste (page) ----
    /// "Your taste" profile page.
    Taste,
}

impl Route {
    /// Sidebar section the route belongs to.
    pub fn section(&self) -> u8 {
        match self {
            Route::Home | Route::Album(_) | Route::Artist(_) | Route::Playlist(_) | Route::ArtistMore { .. } | Route::HomeSection(_) => 0,
            Route::Search => 1,
            Route::Library(_) | Route::LocalPlaylist(_) | Route::LibraryArtist(_) | Route::LibraryAlbum(_) => 2,
            Route::AudioLab => 3,
            Route::Settings => 4,
            Route::Explore | Route::MoodsGenres | Route::Charts | Route::NewReleases | Route::Browse { .. } => 5,
            Route::Stats => 6,
            Route::Taste => 7,
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
