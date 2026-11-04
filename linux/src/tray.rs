//! The icon in the system tray, through the `StatusNotifierItem` interface, which Waybar's tray
//! shows.

use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip, TrayMethods};
use tokio::sync::mpsc;
use zib_core::Channel;

use crate::icon;

/// What the owner asked for through the icon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Activate,
    Quit,
}

struct Tray {
    channel: Channel,
    events: mpsc::UnboundedSender<Event>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        self.channel.app_id().into()
    }

    fn title(&self) -> String {
        self.channel.display_name().into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        [22, 32, 48, 64]
            .into_iter()
            .map(|size| Icon {
                width: size,
                height: size,
                data: icon::pixmap(self.channel, usize::try_from(size).unwrap_or(0)),
            })
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: self.channel.display_name().into(), ..ToolTip::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.events.send(Event::Activate);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: format!("Open {}", self.channel.display_name()),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.events.send(Event::Activate);
                }),
                ..StandardItem::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "_Quit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.events.send(Event::Quit);
                }),
                ..StandardItem::default()
            }
            .into(),
        ]
    }
}

/// Shows the icon, and returns the owner's requests through it, or an error when no tray is
/// available, such as on a machine without a desktop.
pub async fn show(channel: Channel) -> Result<mpsc::UnboundedReceiver<Event>, ksni::Error> {
    let (events, receiver) = mpsc::unbounded_channel();
    let handle = Tray { channel, events }.spawn().await?;
    // The icon stays as long as the process runs.
    std::mem::forget(handle);
    Ok(receiver)
}
