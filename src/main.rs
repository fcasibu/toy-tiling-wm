use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

struct WM {
    // Testing purposes for now
    display_target: String,

    conn: RustConnection,
    root: u32,
    clients: Vec<Window>,
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

        let res = conn.change_window_attributes(root, &change).unwrap().check();

        if let Err(ReplyError::X11Error(error)) = res && error.error_kind == ErrorKind::Access {
            eprintln!("Another wm is already running.");
            std::process::exit(1);
        }

        Self {
            display_target,
            conn,
            root,
            clients: Vec::with_capacity(1024),
            focused: root,
        }
    }

    fn handle_events(&mut self, event: Event) -> Result<(), Box<dyn std::error::Error>> {
        match event {
            Event::MapRequest(event) => {
                self.conn.change_window_attributes(
                    event.window,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::ENTER_WINDOW),
                )?;

                self.conn.map_window(event.window)?;
                self.clients.push(event.window);

                tracing::info!("Clients = {}", self.clients.len());
            }

            Event::KeyPress(_) => {
                // Just test opening any window for now
                std::process::Command::new("alacritty")
                    .env("DISPLAY", &self.display_target)
                    .spawn()?;
            }

            Event::EnterNotify(event) => {
                self.conn
                    .set_input_focus(InputFocus::PARENT, event.event, x11rb::CURRENT_TIME)?;
                self.focused = event.event;
            }

            Event::DestroyNotify(event) => {
                if let Some(index) = self.clients.iter().position(|win| *win == event.window) {
                    self.clients.remove(index);

                    let win = if self.clients.is_empty() {
                        self.root
                    } else {
                        *self
                            .clients
                            .get(index.saturating_sub(1))
                            .expect("should not be out of bounds")
                    };

                    self.focused = win;
                    self.conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)?;
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

    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let mut wm = WM::new();

    const ENTER_KEY_CODE: u8 = 36;
    wm.conn.grab_key(
        true,
        wm.root,
        ModMask::CONTROL,
        ENTER_KEY_CODE,
        GrabMode::ASYNC,
        GrabMode::ASYNC,
    ).expect("should be able to grab key");

    loop {
        wm.conn.flush().ok();

        let event = wm.conn.wait_for_event().expect("should receive event");
        // TODO(fcasibu): handle errors
        wm.handle_events(event).unwrap();
    }
}
