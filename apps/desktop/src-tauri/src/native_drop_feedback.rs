//! Drawn in the AppKit tracking loop, independently of WebView IPC repaint.
use crate::drag_feedback::DragTab;
use objc2::{AnyThread, runtime::AnyObject};
use objc2_app_kit::{
    NSAttributedStringNSStringDrawing, NSBezierPath, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSGraphicsContext, NSLineBreakMode, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSTextAlignment,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString};

pub fn paint(tab: &DragTab, label: &str, count: usize, width: f64, height: f64) {
    if width < 64.0 || height < 60.0 {
        return;
    }
    let rect = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let rounded =
        |r, radius| NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius);
    let blue = |alpha| NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.33, 0.77, alpha);
    NSGraphicsContext::saveGraphicsState_class();
    // Expanded halo makes acquisition visible without changing the hit region
    // or taking over the pointer. Exact same tab rectangle decides the target.
    for (grow, alpha) in [(6.0, 0.12), (3.0, 0.25)] {
        blue(alpha).setFill();
        rounded(
            rect(
                tab.x - grow,
                tab.y - grow,
                tab.width + grow * 2.0,
                tab.height + grow * 2.0,
            ),
            tab.height / 2.0 + grow,
        )
        .fill();
    }
    blue(1.0).setFill();
    rounded(rect(tab.x, tab.y, tab.width, tab.height), tab.height / 2.0).fill();
    NSColor::whiteColor().setStroke();
    let ring = rounded(
        rect(tab.x - 1.0, tab.y - 1.0, tab.width + 2.0, tab.height + 2.0),
        tab.height / 2.0 + 1.0,
    );
    ring.setLineWidth(1.5);
    ring.stroke();
    draw_text(
        label,
        rect(
            tab.x + 4.0,
            tab.y + (tab.height - 17.0) / 2.0,
            tab.width - 8.0,
            17.0,
        ),
        11.0,
    );
    let hint_width = 260.0_f64.min(width - 16.0).max(1.0);
    let hint_x = (tab.x + tab.width / 2.0 - hint_width / 2.0)
        .clamp(8.0, (width - hint_width - 8.0).max(8.0));
    let hint_y = (tab.y + tab.height + 12.0).min((height - 42.0).max(0.0));
    blue(0.97).setFill();
    rounded(rect(hint_x, hint_y, hint_width, 32.0), 10.0).fill();
    let tip_x = (tab.x + tab.width / 2.0).clamp(hint_x + 12.0, hint_x + hint_width - 12.0);
    let tip = NSBezierPath::bezierPath();
    tip.moveToPoint(NSPoint::new(tip_x - 6.0, hint_y + 1.0));
    tip.lineToPoint(NSPoint::new(tip_x, hint_y - 6.0));
    tip.lineToPoint(NSPoint::new(tip_x + 6.0, hint_y + 1.0));
    tip.closePath();
    tip.fill();
    draw_text(
        &format!("松开移入「{label}」 · {count} 项"),
        rect(hint_x + 10.0, hint_y + 8.0, hint_width - 20.0, 18.0),
        12.0,
    );
    NSGraphicsContext::restoreGraphicsState_class();
}

pub fn paint_insertion(x: f64, y: f64, line_height: f64, count: usize, width: f64, height: f64) {
    if ![x, y, line_height, width, height]
        .into_iter()
        .all(f64::is_finite)
        || width < 64.0
        || height < 60.0
    {
        return;
    }
    NSGraphicsContext::saveGraphicsState_class();
    let rect = NSRect::new(NSPoint::new(x - 1.5, y), NSSize::new(3.0, line_height));
    let line = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 1.5, 1.5);
    NSColor::whiteColor().setStroke();
    line.setLineWidth(2.0);
    line.stroke();
    NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.33, 0.77, 1.0).setFill();
    line.fill();
    let hint_width = 156.0_f64.min(width - 16.0);
    let hint_x = (x + 10.0).min(width - hint_width - 8.0).max(8.0);
    let hint_y = y.clamp(4.0, (height - 32.0).max(4.0));
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        NSRect::new(NSPoint::new(hint_x, hint_y), NSSize::new(hint_width, 27.0)),
        9.0,
        9.0,
    )
    .fill();
    draw_text(
        &format!("松开插入 · {count} 项"),
        NSRect::new(
            NSPoint::new(hint_x + 5.0, hint_y + 5.0),
            NSSize::new(hint_width - 10.0, 18.0),
        ),
        12.0,
    );
    NSGraphicsContext::restoreGraphicsState_class();
}

fn draw_text(value: &str, bounds: NSRect, size: f64) {
    let font = NSFont::boldSystemFontOfSize(size);
    let ink = NSColor::whiteColor();
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let attrs = unsafe {
        NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            &[font.as_ref(), ink.as_ref(), paragraph.as_ref()],
        )
    };
    let text = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(value),
            Some(&attrs),
        )
    };
    text.drawInRect(bounds);
}
