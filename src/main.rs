use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use thiserror::Error;

const KEY_ENTER: u8 = 36;
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

#[derive(PartialEq, Debug)]
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

    fn from_geometry(id: Window, geom: &GetGeometryReply) -> Self {
        Self::new(id, geom.x, geom.y, geom.width, geom.height)
    }
}

struct WM {
    // Testing purposes for now
    display_target: String,

    conn: RustConnection,
    screen: Screen,
    root: u32,
    clients: Vec<Client>,
    focused: Window,
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
        }
    }

    fn relayout(&mut self) -> Result<(), WMError> {
        let len = self.clients.len() as u16;

        for (index, client) in self.clients.iter_mut().enumerate() {
            let new_width = self.screen.width_in_pixels / len;
            client.width = new_width;
            client.x = (new_width * index as u16) as i16;

            self.conn.configure_window(
                client.id,
                &ConfigureWindowAux::new()
                    .width(client.width as u32)
                    .height(client.height as u32)
                    .x(client.x as i32),
            )?;

            self.conn.change_window_attributes(
                client.id,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::ENTER_WINDOW),
            )?;
            self.conn.map_window(client.id)?;
        }

        Ok(())
    }

    fn handle_events(&mut self, event: Event) -> Result<(), WMError> {
        match event {
            Event::MapRequest(event) => {
                let client_id = event.window;

                if self.clients.len() < MAX_CLIENTS {
                    self.conn.configure_window(
                        client_id,
                        &ConfigureWindowAux::new()
                            .width(self.screen.width_in_pixels as u32)
                            .height(self.screen.height_in_pixels as u32),
                    )?;

                    let geom = self.conn.get_geometry(client_id)?.reply()?;
                    self.clients.push(Client::from_geometry(client_id, &geom));
                    self.relayout()?;

                    for client in &self.clients {
                        tracing::info!("{client:?}");
                    }

                    self.conn.set_input_focus(
                        InputFocus::PARENT,
                        self.clients.last().map_or(self.root, |c| c.id),
                        x11rb::CURRENT_TIME,
                    )?;
                } else {
                    eprintln!("There can only be {MAX_CLIENTS} clients at once.");

                    self.conn.destroy_window(client_id)?;
                }

                self.conn.flush().ok();

                tracing::info!("Clients = {}", self.clients.len());
            }

            Event::KeyPress(_) => {
                // Just test opening any window for now
                std::process::Command::new("firefox")
                    .env("DISPLAY", &self.display_target)
                    .spawn()?;
            }

            Event::EnterNotify(event) => {
                self.focused = event.event;
            }

            Event::UnmapNotify(event) => {
                self.conn
                    .set_input_focus(InputFocus::PARENT, self.root, x11rb::CURRENT_TIME)?;
                self.focused = self.root;
                tracing::info!(
                    "Window = {}, Event = {}, from_configure = {}",
                    event.window,
                    event.event,
                    event.from_configure
                );
            }

            Event::DestroyNotify(event) => {
                if let Some(index) = self
                    .clients
                    .iter()
                    .position(|client| client.id == event.window)
                {
                    self.clients.remove(index);

                    let win = if self.clients.is_empty() {
                        self.root
                    } else {
                        self.clients
                            .get(index.saturating_sub(1))
                            .expect("should not be out of bounds")
                            .id
                    };

                    self.relayout()?;

                    self.focused = win;
                    self.conn
                        .set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)?;
                }

                tracing::info!("Clients = {}", self.clients.len());
            }

            event => {
                tracing::info!("{event:?}");
            }
        }

        Ok(())
    }
}

fn init_tracing() {
    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

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

    loop {
        wm.conn.flush().ok();

        let event = wm.conn.wait_for_event().unwrap();
        let mut event_option = Some(event);
        while let Some(event) = event_option {
            // TODO(fcasibu): handle errors
            wm.handle_events(event).unwrap();
            event_option = wm.conn.poll_for_event().unwrap();
        }
    }
}
