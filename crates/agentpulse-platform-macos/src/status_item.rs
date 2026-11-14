//! A menu bar status item: a template icon, an optional short title, and a
//! menu of rows (informational, or clickable when they carry an action) above
//! an optional update item, "Open AgentPulse" and "Quit AgentPulse".

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAutoresizingMaskOptions, NSBitmapFormat, NSBitmapImageRep,
    NSCellImagePosition, NSControl, NSDeviceRGBColorSpace, NSFont, NSFontAttributeName,
    NSFontWeightRegular, NSImage, NSImageScaling, NSImageView, NSMenu, NSMenuDelegate, NSMenuItem,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSResponder, NSStatusBar, NSStatusItem,
    NSTextAlignment, NSTextTab, NSVariableStatusItemLength, NSView,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenDashboard,
    /// A session row was clicked; the id is the one its [`MenuLine`] carried.
    OpenSession(String),
    OpenUpdate,
    Quit,
    /// The menu opened or closed, so its rows can be kept current while it is open.
    MenuOpened,
    MenuClosed,
}

/// A menu row: disabled and informational, or clickable when it has an action.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuLine {
    pub text: String,
    /// Right-aligned at [`TRAILING_AT`], in monospaced digits: a duration.
    pub trailing: Option<String>,
    /// Template image left of the text.
    pub image: Option<MenuImage>,
    pub action: Option<MenuAction>,
}

/// Tightly packed RGBA at two pixels per point; macOS uses only the alpha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuImage {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Where a row's trailing column ends, in points from the text's start.
const TRAILING_AT: f64 = 360.0;

/// The title always takes this many digit widths, so the item never changes width.
/// The widest title callers pass: counts cap at "99+". The item's fixed
/// length fits it, and shorter titles are padded to its character count.
pub const WIDEST_TITLE: &str = "99+";
/// Same advance as a digit in a monospaced-digit font, and invisible.
const FIGURE_SPACE: char = '\u{2007}';

struct TargetIvars {
    /// Test-only reason: kept rather than asked of AppKit each time, because the rows test
    /// builds menus off the main thread. The app itself only ever runs this on the main thread.
    mtm: MainThreadMarker,
    on_action: Box<dyn Fn(MenuAction)>,
    /// Latest rows; written into the menu only while it is open.
    lines: RefCell<Vec<MenuLine>>,
    /// The rows currently at the top of the menu, as written by `apply_lines`.
    /// A click reads its action here, by the item's tag.
    shown: RefCell<Vec<MenuLine>>,
    open: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and Target has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            self.ivars().open.set(true);
            self.apply_lines(menu);
            (self.ivars().on_action)(MenuAction::MenuOpened);
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().open.set(false);
            (self.ivars().on_action)(MenuAction::MenuClosed);
        }
    }

    impl Target {
        /// A row with an action; its tag is its index in the rows shown.
        #[unsafe(method(menuRow:))]
        fn menu_row(&self, sender: Option<&AnyObject>) {
            let Some(item) = sender.and_then(|s| s.downcast_ref::<NSMenuItem>()) else {
                return;
            };
            let action = usize::try_from(item.tag())
                .ok()
                .and_then(|i| self.ivars().shown.borrow().get(i)?.action.clone());
            if let Some(action) = action {
                (self.ivars().on_action)(action);
            }
        }

        #[unsafe(method(openDashboard:))]
        fn open_dashboard(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_action)(MenuAction::OpenDashboard);
        }

        #[unsafe(method(openUpdate:))]
        fn open_update(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_action)(MenuAction::OpenUpdate);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_action)(MenuAction::Quit);
        }
    }
);

define_class!(
    /// Shows the animated icon over the button. Changing its image redraws only
    /// this view: the button keeps its length, layout and title untouched. It
    /// ignores the mouse, so clicks still reach the button and open the menu.
    // SAFETY: NSImageView has no subclassing requirements and CatView has no Drop.
    #[unsafe(super(NSImageView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct CatView;

    impl CatView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        // Its frame is set explicitly, so a new image never changes its size.
        // Without this, every frame re-runs the status bar's layout pass.
        #[unsafe(method(invalidateIntrinsicContentSize))]
        fn invalidate_intrinsic_content_size(&self) {}
    }
);

impl Target {
    fn new(mtm: MainThreadMarker, on_action: Box<dyn Fn(MenuAction)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            mtm,
            on_action,
            lines: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
            open: Cell::new(false),
        });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// Brings the rows above the separator up to date with the latest ones.
    /// Existing items are updated in place, and only where their row changed,
    /// so the row under the pointer stays highlighted and unchanged images are
    /// not rebuilt each second while the menu is open.
    fn apply_lines(&self, menu: &NSMenu) {
        let ivars = self.ivars();
        let lines = ivars.lines.borrow();
        let mut shown = ivars.shown.borrow_mut();
        for _ in lines.len()..shown.len() {
            menu.removeItemAtIndex(lines.len() as isize);
        }
        shown.truncate(lines.len());
        for (i, line) in lines.iter().enumerate() {
            if shown.get(i) == Some(line) {
                continue;
            }
            let row = match menu.itemAtIndex(i as isize).filter(|_| i < shown.len()) {
                Some(row) => row,
                None => {
                    let row = menu_item(ivars.mtm, "", None, "");
                    menu.insertItem_atIndex(&row, i as isize);
                    row
                }
            };
            self.fill(&row, i, line);
            match shown.get_mut(i) {
                Some(s) => *s = line.clone(),
                None => shown.push(line.clone()),
            }
        }
    }

    fn fill(&self, row: &NSMenuItem, i: usize, line: &MenuLine) {
        let title = match &line.trailing {
            Some(t) => format!("{}\t{t}", line.text),
            None => line.text.clone(),
        };
        // Never left empty: macOS 26 and later paint an empty item's fallback
        // title "NSMenuItem" (noted by CodexBar, MIT).
        row.setTitle(&NSString::from_str(&title));
        row.setAttributedTitle(line.trailing.is_some().then(|| columns(&title)).as_deref());
        row.setImage(
            line.image
                .as_ref()
                .and_then(|m| {
                    let size = NSSize::new(m.width as f64 / 2.0, m.height as f64 / 2.0);
                    template_image(&m.rgba, m.width, m.height, size)
                })
                .as_deref(),
        );
        row.setTag(i as isize);
        let clickable = line.action.is_some();
        // SAFETY: Target implements menuRow:, and the menu holds its target weakly
        // while StatusItem keeps it alive.
        unsafe {
            row.setAction(clickable.then_some(sel!(menuRow:)));
            row.setTarget(clickable.then_some(&**self as &AnyObject));
        }
        row.setEnabled(clickable);
    }
}

/// The menu font with monospaced digits and a right tab stop at [`TRAILING_AT`].
fn columns(title: &str) -> Retained<NSAttributedString> {
    let size = NSFont::menuFontOfSize(0.0).pointSize();
    // SAFETY: an immutable AppKit constant.
    let weight = unsafe { NSFontWeightRegular };
    let font = NSFont::monospacedDigitSystemFontOfSize_weight(size, weight);
    let style = NSMutableParagraphStyle::new();
    // SAFETY: an empty options dictionary is valid.
    let tab = unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Right,
            TRAILING_AT,
            &NSDictionary::new(),
        )
    };
    style.setTabStops(Some(&NSArray::from_retained_slice(&[tab])));
    // SAFETY: immutable AppKit constants, each with a value of the type it names.
    let attrs = unsafe {
        NSDictionary::<NSString, AnyObject>::from_slices(
            &[NSFontAttributeName, NSParagraphStyleAttributeName],
            &[&*font as &AnyObject, &*style as &AnyObject],
        )
    };
    // SAFETY: the attributes are of the types their keys require.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(title),
            Some(&attrs),
        )
    }
}

pub struct StatusItem {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    /// Menu items and the menu hold their target weakly, so it is kept alive here.
    target: Retained<Target>,
    cat: Retained<CatView>,
    /// "Update Available: …", hidden while there is none.
    update: Retained<NSMenuItem>,
    update_version: RefCell<Option<Option<String>>>,
    /// One image per distinct bitmap, built once.
    // ponytail: unbounded, fine for the few dozen sprite frames; add eviction if callers vary pixels freely.
    images: RefCell<HashMap<Vec<u8>, Retained<NSImage>>>,
    icon: RefCell<Option<Retained<NSImage>>>,
    /// Icon size in points the item length was fitted to.
    fitted: Cell<Option<(f64, f64)>>,
    title: RefCell<Option<String>>,
}

impl StatusItem {
    pub fn new(mtm: MainThreadMarker, on_action: Box<dyn Fn(MenuAction)>) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let target = Target::new(mtm, on_action);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let entries = [
            ("", sel!(openUpdate:), ""),
            ("Open AgentPulse", sel!(openDashboard:), ""),
            ("Quit AgentPulse", sel!(quit:), "q"),
        ]
        .map(|(title, action, key)| {
            let entry = menu_item(mtm, title, Some(action), key);
            // SAFETY: Target implements the action selector.
            unsafe { entry.setTarget(Some(&target)) };
            menu.addItem(&entry);
            entry
        });
        let [update, ..] = entries;
        update.setHidden(true);
        item.setMenu(Some(&menu));
        if let Some(button) = item.button(mtm) {
            // Monospaced digits, so a count takes the same width as the padding.
            let size = NSFont::menuBarFontOfSize(0.0).pointSize();
            // SAFETY: an immutable AppKit constant.
            let weight = unsafe { NSFontWeightRegular };
            button.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                size, weight,
            )));
            button.setImagePosition(NSCellImagePosition::ImageLeft);
            button.setTitle(&NSString::from_str(&padded("")));
            button.setToolTip(Some(ns_string!("AgentPulse")));
        }
        let cat: Retained<CatView> =
            unsafe { msg_send![CatView::alloc(mtm), initWithFrame: NSRect::ZERO] };
        cat.setImageScaling(NSImageScaling::ScaleNone);
        // Stays vertically centred if the menu bar's height changes.
        cat.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinYMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        if let Some(button) = item.button(mtm) {
            button.addSubview(&cat);
        }
        StatusItem {
            mtm,
            item,
            menu,
            target,
            cat,
            update,
            update_version: RefCell::new(None),
            images: RefCell::new(HashMap::new()),
            icon: RefCell::new(None),
            fitted: Cell::new(None),
            title: RefCell::new(None),
        }
    }

    /// Tightly packed RGBA pixels (`width * height * 4`), shown `points_high`
    /// points tall as a template image, so macOS tints it for light and dark
    /// menu bars and for the highlighted state.
    pub fn set_icon(&self, rgba: &[u8], width: usize, height: usize, points_high: f64) {
        assert_eq!(rgba.len(), width * height * 4, "icon size");
        let size = NSSize::new(points_high * width as f64 / height as f64, points_high);
        let cached = self.images.borrow().get(rgba).cloned();
        let Some(image) = cached.or_else(|| {
            let image = template_image(rgba, width, height, size)?;
            self.images
                .borrow_mut()
                .insert(rgba.to_vec(), image.clone());
            Some(image)
        }) else {
            return;
        };
        if self
            .icon
            .borrow()
            .as_ref()
            .is_some_and(|i| std::ptr::eq(&**i, &*image))
        {
            return;
        }
        let key = (size.width, size.height);
        if self.fitted.get() != Some(key)
            && let Some(button) = self.item.button(self.mtm)
            && let Some(blank) = template_image(&vec![0; rgba.len()], width, height, size)
        {
            // The button holds a blank image of the icon's size, which places
            // the title, and the cat view sits on it. The fixed length fits the
            // icon and the widest title, so neither poses nor counts move other
            // menu bar items, and the cat view keeps its place.
            button.setImage(Some(&blank));
            button.setTitle(&NSString::from_str(WIDEST_TITLE));
            button.sizeToFit();
            self.item.setLength(button.frame().size.width);
            if let Some(cell) = button.cell() {
                self.cat.setFrame(cell.imageRectForBounds(button.bounds()));
            }
            let title = padded(self.title.borrow().as_deref().unwrap_or(""));
            button.setTitle(&NSString::from_str(&title));
            self.fitted.set(Some(key));
        }
        self.cat.setImage(Some(&image));
        *self.icon.borrow_mut() = Some(image);
    }

    /// Text next to the icon, such as "2"; an empty string hides it. Up to
    /// [`WIDEST_TITLE`] fits; the space is reserved even when hidden.
    pub fn set_title(&self, title: &str) {
        if !replace_if_changed(&self.title, title.to_owned()) {
            return;
        }
        if let Some(button) = self.item.button(self.mtm) {
            button.setTitle(&NSString::from_str(&padded(title)));
        }
    }

    /// Replaces the rows above the separator. They are written into the menu
    /// when it opens, and live while it stays open.
    pub fn set_lines(&self, lines: &[MenuLine]) {
        let target = &self.target;
        if *target.ivars().lines.borrow() == lines {
            return;
        }
        *target.ivars().lines.borrow_mut() = lines.to_vec();
        if target.ivars().open.get() {
            target.apply_lines(&self.menu);
        }
    }

    /// What VoiceOver says for the item, such as "AgentPulse, 1 waiting".
    pub fn set_accessibility_label(&self, label: &str) {
        if let Some(button) = self.item.button(self.mtm) {
            button.setAccessibilityLabel(Some(&NSString::from_str(label)));
        }
    }

    /// Shows "Update Available: AgentPulse <version>…", or hides it with None.
    pub fn set_update(&self, version: Option<&str>) {
        if !replace_if_changed(&self.update_version, version.map(str::to_owned)) {
            return;
        }
        if let Some(v) = version {
            self.update.setTitle(&NSString::from_str(&format!(
                "Update Available: AgentPulse {v}…"
            )));
        }
        self.update.setHidden(version.is_none());
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

fn template_image(
    rgba: &[u8],
    width: usize,
    height: usize,
    size: NSSize,
) -> Option<Retained<NSImage>> {
    let (w, h) = (width as isize, height as isize);
    // SAFETY: null planes make the rep allocate its own buffer of
    // `bytesPerRow * height` bytes in the layout described.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::AlphaNonpremultiplied,
            w * 4,
            32,
        )
    }?;
    // SAFETY: the buffer is exactly `rgba.len()` bytes (rows are not padded
    // because bytesPerRow was given) and does not overlap `rgba`.
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), rep.bitmapData(), rgba.len()) };
    rep.setSize(size);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);
    image.setTemplate(true);
    Some(image)
}

/// Pads with figure spaces to the character count of [`WIDEST_TITLE`].
fn padded(title: &str) -> String {
    let pad = WIDEST_TITLE
        .chars()
        .count()
        .saturating_sub(title.chars().count());
    title
        .chars()
        .chain(std::iter::repeat_n(FIGURE_SPACE, pad))
        .collect()
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    // SAFETY: the action, if any, is implemented by the target set by the caller.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

/// Stores `value` and returns true when it differs from the stored value.
fn replace_if_changed<T: PartialEq>(last: &RefCell<Option<T>>, value: T) -> bool {
    let mut last = last.borrow_mut();
    if last.as_ref() == Some(&value) {
        return false;
    }
    *last = Some(value);
    true
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    /// Title, displayed title, enabled, clickable, image size in points, tag.
    fn dump(menu: &NSMenu, rows: usize) -> Vec<String> {
        (0..rows as isize)
            .map(|i| {
                let item = menu.itemAtIndex(i).unwrap();
                let shown = item.attributedTitle().map_or("plain".into(), |a| {
                    format!("attributed {:?}", a.string().to_string())
                });
                let image = item
                    .image()
                    .map(|m| format!("{}x{} pt", m.size().width, m.size().height));
                format!(
                    "{:?} | {shown} | enabled {} | action {} | image {} | tag {}",
                    item.title().to_string(),
                    item.isEnabled(),
                    item.action().is_some(),
                    image.as_deref().unwrap_or("none"),
                    item.tag()
                )
            })
            .collect()
    }

    #[test]
    fn rows_become_menu_items_and_update_in_place() {
        // Test only: libtest runs tests off the main thread. AppKit's menu
        // objects work there as long as nothing else touches them and no menu
        // is shown; the app itself always has the real marker.
        // SAFETY: as above.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let clicked = Rc::new(RefCell::new(Vec::new()));
        let sink = clicked.clone();
        let target = Target::new(mtm, Box::new(move |a| sink.borrow_mut().push(a)));
        let menu = NSMenu::new(mtm);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let image = MenuImage {
            rgba: vec![0; 80 * 20 * 4],
            width: 80,
            height: 20,
        };
        let rows = vec![
            MenuLine {
                text: "Updated just now".into(),
                ..MenuLine::default()
            },
            MenuLine {
                text: "Codex 5h · 62% used".into(),
                image: Some(image),
                ..MenuLine::default()
            },
            MenuLine {
                text: "◆  api · Codex · Waiting".into(),
                trailing: Some("46m 00s".into()),
                action: Some(MenuAction::OpenSession("codex/s1".into())),
                ..MenuLine::default()
            },
        ];
        *target.ivars().lines.borrow_mut() = rows;
        target.apply_lines(&menu);
        let first = dump(&menu, 3);
        println!("{}", first.join("\n"));
        assert_eq!(
            first,
            [
                r#""Updated just now" | plain | enabled false | action false | image none | tag 0"#,
                r#""Codex 5h · 62% used" | plain | enabled false | action false | image 40x10 pt | tag 1"#,
                r#""◆  api · Codex · Waiting\t46m 00s" | attributed "◆  api · Codex · Waiting\t46m 00s" | enabled true | action true | image none | tag 2"#,
            ]
        );
        assert_eq!(menu.numberOfItems(), 4, "three rows above the separator");
        let session = menu.itemAtIndex(2).unwrap();
        // SAFETY: Target implements menuRow:, which takes the clicked item.
        let () = unsafe { msg_send![&*target, menuRow: &*session] };
        assert_eq!(
            *clicked.borrow(),
            [MenuAction::OpenSession("codex/s1".into())]
        );
        // New rows not yet written: a click still opens what the item shows.
        target.ivars().lines.borrow_mut()[2].action = Some(MenuAction::OpenDashboard);
        let () = unsafe { msg_send![&*target, menuRow: &*session] };
        assert_eq!(
            clicked.borrow()[1],
            MenuAction::OpenSession("codex/s1".into())
        );
        target.ivars().lines.borrow_mut()[2].action =
            Some(MenuAction::OpenSession("codex/s1".into()));
        // Unchanged rows are left alone: the bar keeps its image object.
        let bar = menu.itemAtIndex(1).unwrap().image().unwrap();
        target.apply_lines(&menu);
        let again = menu.itemAtIndex(1).unwrap().image().unwrap();
        assert!(std::ptr::eq(&*bar, &*again), "not rebuilt");

        // Fewer rows: the same items are reused, the extra one removed.
        *target.ivars().lines.borrow_mut() = vec![MenuLine {
            text: "Data collection stopped".into(),
            ..MenuLine::default()
        }];
        target.apply_lines(&menu);
        let kept = menu.itemAtIndex(0).unwrap();
        assert_eq!(menu.numberOfItems(), 2);
        assert_eq!(
            dump(&menu, 1),
            [
                r#""Data collection stopped" | plain | enabled false | action false | image none | tag 0"#
            ]
        );
        target.apply_lines(&menu);
        assert!(
            std::ptr::eq(&*kept, &*menu.itemAtIndex(0).unwrap()),
            "updated in place"
        );
    }

    #[test]
    fn the_longest_session_row_ends_before_the_duration_column() {
        use objc2_app_kit::NSAttributedStringNSStringDrawing;
        // The app's longest row: a name cut to 16 characters (the app's cap),
        // the longest agent and state names, and wide letters.
        let text = "◆  WWagentpulse-g… · Claude Code · Waiting";
        let width = columns(text).size().width;
        // Room for a gap and a right-aligned "23h 59m" before the tab stop.
        let duration = columns("23h 59m").size().width;
        println!("longest row: {width:.1} pt + {duration:.1} pt duration, tab at {TRAILING_AT} pt");
        assert!(
            width + 12.0 + duration <= TRAILING_AT,
            "{width} + {duration} overflows {TRAILING_AT}"
        );
    }

    #[test]
    fn unchanged_values_are_not_reapplied() {
        let last = RefCell::new(None);
        assert!(replace_if_changed(&last, "2".to_string()));
        assert!(!replace_if_changed(&last, "2".to_string()));
        assert!(replace_if_changed(&last, String::new()));
        assert!(!replace_if_changed(&last, String::new()));
    }

    #[test]
    fn titles_keep_a_constant_width() {
        assert_eq!(padded(""), "\u{2007}\u{2007}\u{2007}");
        assert_eq!(padded("2"), "2\u{2007}\u{2007}");
        assert_eq!(padded("12"), "12\u{2007}");
        assert_eq!(padded(WIDEST_TITLE), WIDEST_TITLE);
    }
}
