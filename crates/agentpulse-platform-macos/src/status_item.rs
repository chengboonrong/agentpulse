//! A menu bar status item: a template icon, an optional short title, and a
//! menu of informational rows above "Open AgentPulse" and "Quit AgentPulse".

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBitmapFormat, NSBitmapImageRep, NSCellImagePosition, NSControl,
    NSDeviceRGBColorSpace, NSFont, NSFontWeightRegular, NSImage, NSImageScaling, NSImageView,
    NSMenu, NSMenuDelegate, NSMenuItem, NSResponder, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength, NSView,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, ns_string};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenDashboard,
    Quit,
}

/// An informational, disabled menu row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuLine {
    pub text: String,
}

/// The title always takes this many digit widths, so the item never changes width.
/// The widest title callers pass: counts cap at "99+". The item's fixed
/// length fits it, and shorter titles are padded to its character count.
pub const WIDEST_TITLE: &str = "99+";
/// Same advance as a digit in a monospaced-digit font, and invisible.
const FIGURE_SPACE: char = '\u{2007}';

struct TargetIvars {
    on_action: Box<dyn Fn(MenuAction)>,
    /// Latest informational rows; written into the menu only while it is open.
    lines: RefCell<Vec<MenuLine>>,
    /// Informational rows currently at the top of the menu.
    rows: Cell<usize>,
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
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().open.set(false);
        }
    }

    impl Target {
        #[unsafe(method(openDashboard:))]
        fn open_dashboard(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_action)(MenuAction::OpenDashboard);
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
            on_action,
            lines: RefCell::new(Vec::new()),
            rows: Cell::new(0),
            open: Cell::new(false),
        });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// Replaces the informational rows above the separator with the latest ones.
    fn apply_lines(&self, menu: &NSMenu) {
        let ivars = self.ivars();
        for _ in 0..ivars.rows.get() {
            menu.removeItemAtIndex(0);
        }
        let lines = ivars.lines.borrow();
        for (i, line) in lines.iter().enumerate() {
            let row = menu_item(self.mtm(), &line.text, None, "");
            row.setEnabled(false);
            menu.insertItem_atIndex(&row, i as isize);
        }
        ivars.rows.set(lines.len());
    }
}

pub struct StatusItem {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    /// Menu items and the menu hold their target weakly, so it is kept alive here.
    target: Retained<Target>,
    cat: Retained<CatView>,
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
        for (title, action, key) in [
            ("Open AgentPulse", sel!(openDashboard:), ""),
            ("Quit AgentPulse", sel!(quit:), "q"),
        ] {
            let entry = menu_item(mtm, title, Some(action), key);
            // SAFETY: Target implements the action selector.
            unsafe { entry.setTarget(Some(&target)) };
            menu.addItem(&entry);
        }
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

    /// Replaces the informational rows above the separator. They are written
    /// into the menu when it opens, and live while it stays open.
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
    use super::*;

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
