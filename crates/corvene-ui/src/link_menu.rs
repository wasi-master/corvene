//! Corvene `355-github-links-in-app`: a click on a link in rendered markdown.
//! A link to a release, comparison, commit, issue or pull request of a
//! repository in the list asks (a menu at the pointer) whether to show it in
//! the app or on GitHub; anything else opens in the browser as in GitHub
//! Desktop. Issue references in Markdown are links too
//! (`corvene_core::markdown_filters`).

use corvene_core::github_links::{link_description, parse_github_link, repository_for_link};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::*;

use crate::context_menu::MenuItem;

/// Open `url`, offering the in-app view when there is one.
pub fn open_link(url: String, window: &mut Window, cx: &mut App) {
    let target = parse_github_link(&url).and_then(|link| {
        let s = AppState::try_global(cx)?.read(cx);
        let id = repository_for_link(s, &link)?;
        Some((id, link_description(s, id, &link), link))
    });
    let Some((id, description, link)) = target else {
        Dispatcher::open_url(&url, cx);
        return;
    };
    let in_app = format!("Show {description} in Corvene");
    let browser_url = url.clone();
    let items = vec![
        MenuItem::new(in_app, move |_, cx| {
            Dispatcher::open_github_link(id, link.clone(), url.clone(), cx)
        }),
        MenuItem::new("Open on GitHub", move |_, cx| {
            Dispatcher::open_url(&browser_url, cx)
        }),
    ];
    let position = window.mouse_position();
    crate::native_menu::show_context_menu(items, position, window, cx);
}
