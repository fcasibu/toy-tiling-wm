use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use thiserror::Error;

const KEY_ENTER: u8 = 36;
const KEY_E: u8 = 26;
const MAX_CLIENTS: usize = 256;

#[derive(Error, Debug)]
pub enum WMError {
    #[error("connection error")]
    ConnectionError(#[from] x11rb::errors::ConnectionError),

    #[error("reply error")]
    ReplyError(#[from] x11rb::errors::ReplyError),

    #[error("standard error")]
    StandardError(#[from] std::io::Error),
}

#[derive(PartialEq, Debug, Copy, Clone)]
struct Client {
    id: Window,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

impl Client {
    fn new(id: Window, x: i16, y: i16, width: u16, height: u16) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
        }
    }
}

enum Layout {
    Split,
    MasterStack,
}

struct WM {
    // Testing purposes for now
    display_target: String,

    conn: RustConnection,
    screen: Screen,
    root: u32,
    clients: Vec<Client>,
    focused: Window,
    layout: Layout,
    should_relayout: bool,
}

impl WM {
    fn new() -> Self {
        let display_target = std::env::var("DISPLAY").unwrap_or(String::from(":0"));
        let (conn, screen_num) =
            x11rb::connect(None).expect("should be able to connect to an x11 server");
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;

        let change = ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY);

        let res = conn
            .change_window_attributes(root, &change)
            .unwrap()
            .check();

        if let Err(ReplyError::X11Error(error)) = res
            && error.error_kind == ErrorKind::Access
        {
            eprintln!("Another wm is already running.");
            std::process::exit(1);
        }

        Self {
            display_target,
            screen: screen.clone(),
            conn,
            root,
            clients: Vec::with_capacity(MAX_CLIENTS),
            focused: root,
            layout: Layout::MasterStack,
            should_relayout: true,
        }
    }

    fn relayout(&mut self) -> Result<(), WMError> {
        let len = self.clients.len() as u16;
        if len == 0 {
            return Ok(());
        }

        let screen_w = self.screen.width_in_pixels;
        let screen_h = self.screen.height_in_pixels;

        for (index, client) in self.clients.iter_mut().enumerate() {
            let old = *client;

            match self.layout {
                Layout::Split => {
                    let new_width = screen_w / len;
                    client.width = new_width;
                    client.height = screen_h;
                    client.x = (new_width * index as u16) as i16;
                    client.y = 0;
                }

                Layout::MasterStack => {
                    if len == 1 {
                        client.width = screen_w;
                        client.height = screen_h;
                        client.x = 0;
                        client.y = 0;
                    } else {
                        let half_width = screen_w / 2;
                        client.width = half_width;

                        if index == 0 {
                            client.height = screen_h;
                            client.x = 0;
                            client.y = 0;
                        } else {
                            let stack_count = len - 1;
                            let new_height = screen_h / stack_count;
                            client.height = new_height;
                            client.x = half_width as i16;
                            client.y = (new_height * (index - 1) as u16) as i16;
                        }
                    }
                }
            }

            if old != *client {
                self.conn.configure_window(
                    client.id,
                    &ConfigureWindowAux::new()
                        .width(client.width as u32)
                        .height(client.height as u32)
                        .x(client.x as i32)
                        .y(client.y as i32),
                )?;
            }
        }

        Ok(())
    }

    fn handle_events(&mut self, event: Event) -> Result<(), WMError> {
        match event {
            Event::MapRequest(event) => {
                let client_id = event.window;

                if self.clients.len() < MAX_CLIENTS {
                    self.conn.change_window_attributes(
                        client_id,
                        &ChangeWindowAttributesAux::new().event_mask(EventMask::ENTER_WINDOW),
                    )?;

                    let client = Client::new(client_id, 0, 0, 0, 0);
                    let focused_index = self.clients.iter().position(|c| c.id == self.focused);

                    if let Some(index) = focused_index
                        && index + 1 < self.clients.len()
                    {
                        self.clients.insert(index + 1, client);
                    } else {
                        self.clients.push(client);
                    }

                    self.conn.map_window(client_id)?;
                    self.should_relayout = true;
                } else {
                    eprintln!("There can only be {MAX_CLIENTS} clients at once.");

                    self.conn.destroy_window(client_id)?;
                }

                tracing::info!("Clients = {}", self.clients.len());
            }

            Event::KeyPress(event) => {
                // Just test opening any window for now

                match event.detail {
                    KEY_E => {
                        self.layout = match self.layout {
                            Layout::Split => Layout::MasterStack,
                            Layout::MasterStack => Layout::Split,
                        };
                        self.should_relayout = true;
                    }

                    KEY_ENTER => {
                        std::process::Command::new("firefox")
                            .env("DISPLAY", &self.display_target)
                            .spawn()?;
                        self.should_relayout = true;
                    }

                    _ => {}
                }
            }

            Event::EnterNotify(event) => {
                self.focused = event.event;

                if let Ok(Ok(attr)) = self
                    .conn
                    .get_window_attributes(self.focused)
                    .map(|r| r.reply())
                    && attr.map_state != MapState::UNMAPPED
                {
                    self.conn.set_input_focus(
                        InputFocus::PARENT,
                        self.focused,
                        x11rb::CURRENT_TIME,
                    )?;
                }
            }

            Event::UnmapNotify(event) => {
                self.clients.retain(|c| c.id != event.window);
                self.should_relayout = true;
            }

            Event::DestroyNotify(event) => {
                self.clients.retain(|c| c.id != event.window);
                self.should_relayout = true;
            }

            event => {
                tracing::info!("{event:?}");
            }
        }

        Ok(())
    }
}

fn main() {
    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut wm = WM::new();

    wm.conn
        .grab_key(
            true,
            wm.root,
            ModMask::CONTROL,
            KEY_ENTER,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .expect("should be able to grab key");

    wm.conn
        .grab_key(
            true,
            wm.root,
            ModMask::CONTROL,
            KEY_E,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .expect("should be able to grab key");

    // TODO(fcasibu): handle errors
    loop {
        wm.conn.flush().ok();

        let event = wm.conn.wait_for_event().unwrap();
        let mut event_option = Some(event);
        while let Some(event) = event_option {
            wm.handle_events(event).unwrap();
            event_option = wm.conn.poll_for_event().unwrap();
        }
        if wm.should_relayout {
            wm.relayout().unwrap();
            wm.should_relayout = false;
        }
    }
}
