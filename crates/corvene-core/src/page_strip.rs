//! Corvene `1359-page-strip`: the settings of the page diff's thumbnail
//! strip (PDF files, document and LaTeX Pages views) and the EPUB chapter
//! list. GitHub Desktop shows these files as binary
//! (`app/src/ui/diff/binary-file.tsx`).

use crate::dispatcher::Dispatcher;
use crate::host::Host;

impl Dispatcher {
    pub fn set_page_strip_width(width: f32, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.page_strip_width = width);
    }

    pub fn set_page_strip_changed_only(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.page_strip_changed_only = on);
    }
}
