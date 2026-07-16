use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use thiserror::Error;

use std::collections::HashSet;

/* ---------------------------------------------------------- */
/*                         CONSTANTS                          */
/* ---------------------------------------------------------- */

const KEY_ENTER: u8 = 36;
const KEY_E: u8 = 26;
const KEY_Q: u8 = 24;
const KEY_F: u8 = 41;
const KEY_H: u8 = 43;
const KEY_J: u8 = 44;
const KEY_K: u8 = 45;
const KEY_L: u8 = 46;
const MAX_CLIENTS: usize = 256;
const TITLEBAR_HEIGHT: u16 = 28;
const BG_COLOR: u32 = 0x2B3E75F;
const BORDER_WIDTH: u16 = 2;

/* ---------------------------------------------------------- */
/*                           ENUMS                            */
/* ---------------------------------------------------------- */

#[derive(Error, Debug)]
pub enum WMError {
    #[error("connection error")]
    ConnectionError(#[from] x11rb::errors::ConnectionError),

    #[error("reply error")]
    ReplyError(#[from] x11rb::errors::ReplyError),

    #[error("reply or id error")]
    ReplyOrIdError(#[from] x11rb::errors::ReplyOrIdError),

    #[error("standard error")]
    StandardError(#[from] std::io::Error),
}

#[derive(PartialEq, Copy, Clone)]
enum Layout {
    Split,
    MasterStack,
    Monocle,
}

impl Layout {
    fn apply(&self, client: &mut Client, screen: &Screen, len: u16, index: usize) {
        let screen_w = screen.width_in_pixels;
        let screen_h = screen.height_in_pixels;

        match self {
            Layout::Split => {
                let new_width = screen_w / len;
                client.width = new_width;
                client.height = screen_h;
                client.x = (new_width * index as u16) as i16;
                client.y = 0;
            }

            Layout::Monocle => {
                client.width = screen_w;
                client.height = screen_h;
                client.x = 0;
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
    }
}

enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/* ---------------------------------------------------------- */
/*                          STRUCTS                           */
/* ---------------------------------------------------------- */

#[derive(PartialEq, Debug, Copy, Clone)]
struct Client {
    frame_window: Window,
    window: Window,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

impl Client {
    fn new(window: Window, frame_window: Window, geom: &GetGeometryReply) -> Self {
        Self {
            frame_window,
            window,
            x: geom.x,
            y: geom.y,
            width: geom.width,
            height: geom.height,
        }
    }
}

struct WM {
    // Testing purposes for now
    display_target: String,

    conn: RustConnection,
    gc: Gcontext,
    screen_num: usize,
    root: u32,
    clients: Vec<Client>,
    floating_clients: Vec<Client>,
    layout: Layout,
    prev_layout: Layout,
    should_relayout: bool,
    pending_expose: HashSet<Window>,
    dragged_window: Option<(Window, (i16, i16))>,
}

impl WM {
    fn new() -> Result<Self, WMError> {
        let display_target = std::env::var("DISPLAY").unwrap_or(String::from(":0"));
        let (conn, screen_num) = x11rb::connect(None).unwrap();
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let gc_id = conn.generate_id()?;
        let font_id = conn.generate_id()?;

        conn.open_font(font_id, b"fixed")?;
        conn.create_gc(
            gc_id,
            screen.root,
            &CreateGCAux::new()
                .graphics_exposures(0)
                .background(BG_COLOR)
                .foreground(screen.white_pixel)
                .font(font_id),
        )?;
        conn.close_font(font_id)?;

        let change = ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY);

        let res = conn
            .change_window_attributes(root, &change)
            .unwrap()
            .check();

        if let Err(ReplyError::X11Error(error)) = res
            && error.error_kind == ErrorKind::Access
        {
            tracing::error!("Another wm is already running.");
            std::process::exit(1);
        }

        Ok(Self {
            display_target,
            screen_num,
            gc: gc_id,
            conn,
            root,
            clients: Vec::with_capacity(MAX_CLIENTS),
            floating_clients: Vec::with_capacity(MAX_CLIENTS),
            pending_expose: HashSet::default(),
            layout: Layout::Split,
            prev_layout: Layout::Split,
            should_relayout: true,
            dragged_window: None,
        })
    }

    fn step(&mut self) -> Result<(), WMError> {
        self.refresh()?;
        self.conn.flush()?;

        let event = self.conn.wait_for_event()?;
        let mut event_option = Some(event);

        while let Some(event) = event_option {
            self.handle_events(event)?;
            event_option = self.conn.poll_for_event()?;
        }

        if self.should_relayout {
            self.relayout()?;
            self.should_relayout = false;
        }

        Ok(())
    }

    fn init_keymaps(&self) -> Result<(), WMError> {
        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_ENTER,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_Q,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_E,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_F,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL | ModMask::SHIFT,
            KEY_F,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_H,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_L,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_J,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;
        self.conn.grab_key(
            true,
            self.root,
            ModMask::CONTROL,
            KEY_K,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        Ok(())
    }

    fn get_focused_client(&self) -> Result<Option<&Client>, WMError> {
        let focused_window = self.conn.get_input_focus()?.reply()?.focus;
        Ok(self.find_client_by_id(focused_window))
    }

    fn focus_direction(&self, dir: Direction, time: Timestamp) -> Result<(), WMError> {
        if let Some(client) = self.get_focused_client()? {
            let (fx, fy) = center(client);

            let window = self
                .clients
                .iter()
                .filter(|c| c.window != client.window)
                .filter(|c| {
                    let (cx, cy) = center(c);

                    match dir {
                        Direction::Left => cx < fx,
                        Direction::Right => cx > fx,
                        Direction::Up => cy < fy,
                        Direction::Down => cy > fy,
                    }
                })
                .min_by_key(|c| {
                    let (cx, cy) = center(c);
                    match dir {
                        Direction::Left | Direction::Right => (cx - fx).abs() + (cy - fy).abs() * 2,
                        Direction::Up | Direction::Down => (cy - fy).abs() + (cx - fx).abs() * 2,
                    }
                })
                .map(|c| c.window);

            if let Some(window) = window {
                self.conn
                    .set_input_focus(InputFocus::PARENT, window, time)?;
            }
        }

        Ok(())
    }

    fn draw_titlebar(&self, client: &Client) -> Result<(), WMError> {
        let reply = self
            .conn
            .get_property(
                false,
                client.window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                0,
                u32::MAX,
            )?
            .reply()?;

        let rect_gc_id = self.conn.generate_id()?;

        self.conn.create_gc(
            rect_gc_id,
            client.frame_window,
            &CreateGCAux::new().foreground(BG_COLOR),
        )?;

        self.conn.poly_fill_rectangle(
            client.frame_window,
            rect_gc_id,
            &[Rectangle {
                x: 0,
                y: 0,
                width: client.width,
                height: client.height,
            }],
        )?;

        self.conn.image_text8(
            client.frame_window,
            self.gc,
            4,
            (TITLEBAR_HEIGHT / 2) as i16,
            &reply.value,
        )?;

        Ok(())
    }

    fn refresh(&mut self) -> Result<(), WMError> {
        while let Some(&win) = self.pending_expose.iter().next() {
            self.pending_expose.remove(&win);

            if let Some(client) = self.find_client_by_id(win)
                && let Err(err) = self.draw_titlebar(client)
            {
                tracing::error!(%err, "Failed to draw titlebar.");
            }
        }

        Ok(())
    }

    fn find_client_by_id(&self, id: Window) -> Option<&Client> {
        self.clients
            .iter()
            .find(|c| c.window == id || c.frame_window == id)
            .or_else(|| self.find_floating_client_by_id(id))
    }

    fn find_floating_client_by_id(&self, id: Window) -> Option<&Client> {
        self.floating_clients
            .iter()
            .find(|c| c.window == id || c.frame_window == id)
    }

    fn create_frame_window(&self, geom: &GetGeometryReply) -> Result<Window, WMError> {
        let setup = self.conn.setup();
        assert!(self.screen_num < setup.roots.len());
        let screen = &setup.roots[self.screen_num];

        let win_id = self.conn.generate_id()?;
        self.conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win_id,
            self.root,
            geom.x,
            geom.y,
            geom.width,
            geom.height + TITLEBAR_HEIGHT,
            BORDER_WIDTH,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(screen.white_pixel)
                .border_pixel(BG_COLOR)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::SUBSTRUCTURE_NOTIFY
                        | EventMask::SUBSTRUCTURE_REDIRECT
                        | EventMask::ENTER_WINDOW
                        | EventMask::POINTER_MOTION
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE,
                ),
        )?;

        Ok(win_id)
    }

    fn scan_and_manage_windows(&mut self) -> Result<(), WMError> {
        let tree = self.conn.query_tree(self.root)?.reply()?;

        let mut attributes: Vec<(Window, GetGeometryReply, GetWindowAttributesReply)> =
            Vec::with_capacity(tree.children_len() as usize);

        for child in tree.children {
            let attr = self.conn.get_window_attributes(child)?.reply()?;
            let geom = self.conn.get_geometry(child)?.reply()?;
            attributes.push((child, geom, attr));
        }

        for (win, geom, attr) in attributes {
            if !attr.override_redirect && attr.map_state != MapState::UNMAPPED {
                self.manage_window(win, &geom)?;
            }
        }

        Ok(())
    }

    fn manage_window(&mut self, window: Window, geom: &GetGeometryReply) -> Result<(), WMError> {
        let frame_window = self.create_frame_window(geom)?;

        self.conn
            .reparent_window(window, frame_window, 0, TITLEBAR_HEIGHT as _)?;
        self.conn.map_window(window)?;
        self.conn.map_window(frame_window)?;

        self.clients.push(Client::new(window, frame_window, geom));

        Ok(())
    }

    fn relayout(&mut self) -> Result<(), WMError> {
        let len = self.clients.len() as u16;
        if len == 0 {
            return Ok(());
        }

        let setup = self.conn.setup();
        assert!(self.screen_num < setup.roots.len());
        let screen = &setup.roots[self.screen_num];

        for (index, client) in self.clients.iter_mut().enumerate() {
            self.layout.apply(client, screen, len, index);

            let client_width = client.width.saturating_sub(BORDER_WIDTH);
            let client_height = client.height.saturating_sub(BORDER_WIDTH);

            self.conn.configure_window(
                client.frame_window,
                &ConfigureWindowAux::new()
                    .width(client_width as u32)
                    .height(client_height as u32)
                    .x(client.x as i32)
                    .y(client.y.saturating_add(BORDER_WIDTH as i16) as i32),
            )?;

            self.conn.configure_window(
                client.window,
                &ConfigureWindowAux::new()
                    .width(client_width as u32)
                    .height(client_height.saturating_sub(TITLEBAR_HEIGHT) as u32)
                    .x(0)
                    .y(TITLEBAR_HEIGHT as i32),
            )?;
        }

        Ok(())
    }

    /* ---------------------------------------------------------- */
    /*                       EVENT HANDLERS                       */
    /* ---------------------------------------------------------- */

    fn handle_events(&mut self, event: Event) -> Result<(), WMError> {
        match event {
            Event::Expose(event) => self.handle_expose_event(event)?,
            Event::ConfigureRequest(event) => self.handle_configure_request_event(event)?,
            Event::MapRequest(event) => self.handle_map_request_event(event)?,
            Event::KeyPress(event) => self.handle_keypress_event(event)?,
            Event::EnterNotify(event) => self.handle_enter_notify_event(event)?,
            Event::UnmapNotify(event) => self.handle_unmap_notify_event(event)?,
            Event::MotionNotify(event) => self.handle_motion_notify_event(event)?,
            Event::ButtonPress(event) => self.handle_button_press_event(event)?,
            Event::ButtonRelease(event) => self.handle_button_release_event(event)?,

            event => tracing::info!("{event:?}"),
        }

        Ok(())
    }

    fn handle_expose_event(&mut self, event: ExposeEvent) -> Result<(), WMError> {
        self.pending_expose.insert(event.window);

        Ok(())
    }

    fn handle_configure_request_event(&self, event: ConfigureRequestEvent) -> Result<(), WMError> {
        if self.find_client_by_id(event.window).is_none() {
            self.conn.configure_window(
                event.window,
                &ConfigureWindowAux::from_configure_request(&event)
                    .stack_mode(None)
                    .sibling(None),
            )?;
        }

        Ok(())
    }

    fn handle_map_request_event(&mut self, event: MapRequestEvent) -> Result<(), WMError> {
        if self.clients.len() < MAX_CLIENTS {
            let geom = self.conn.get_geometry(event.window)?.reply()?;
            self.manage_window(event.window, &geom)?;
        } else {
            tracing::error!("There can only be {MAX_CLIENTS} clients at once.");

            self.conn.destroy_window(event.window)?;
        }

        self.should_relayout = true;
        Ok(())
    }

    fn handle_enter_notify_event(&mut self, event: EnterNotifyEvent) -> Result<(), WMError> {
        if let Some(client) = self.find_client_by_id(event.event) {
            self.conn
                .set_input_focus(InputFocus::PARENT, client.window, x11rb::CURRENT_TIME)?;
        }

        Ok(())
    }

    fn handle_unmap_notify_event(&mut self, event: UnmapNotifyEvent) -> Result<(), WMError> {
        if event.event == self.root {
            return Ok(());
        }

        self.clients.retain(|c| {
            if c.window != event.window && c.frame_window != event.window {
                return true;
            }

            self.conn
                .reparent_window(c.window, self.root, c.x, c.y)
                .unwrap();
            self.conn.destroy_window(c.frame_window).unwrap();

            false
        });

        self.floating_clients.retain(|c| {
            if c.window != event.window && c.frame_window != event.window {
                return true;
            }

            self.conn
                .reparent_window(c.window, self.root, c.x, c.y)
                .unwrap();
            self.conn.destroy_window(c.frame_window).unwrap();

            false
        });

        if self.clients.is_empty() && self.floating_clients.is_empty() {
            self.conn
                .set_input_focus(InputFocus::PARENT, self.root, x11rb::CURRENT_TIME)?;
        }

        self.should_relayout = true;
        Ok(())
    }

    fn handle_motion_notify_event(&mut self, event: MotionNotifyEvent) -> Result<(), WMError> {
        if event.state == KeyButMask::BUTTON1
            && let Some((window, (offset_x, offset_y))) = self.dragged_window
        {
            self.conn.configure_window(
                window,
                &ConfigureWindowAux::new()
                    .x((offset_x + event.root_x) as i32)
                    .y((offset_y + event.root_y) as i32),
            )?;
        }

        Ok(())
    }

    fn handle_button_press_event(&mut self, event: ButtonPressEvent) -> Result<(), WMError> {
        if event.detail != 1 {
            return Ok(());
        }

        if let Some(floating_client) = self.find_floating_client_by_id(event.event) {
            {
                let (x, y) = (-event.event_x, -event.event_y);

                self.dragged_window = Some((floating_client.frame_window, (x, y)));
            }
        }

        Ok(())
    }

    fn handle_button_release_event(&mut self, event: ButtonReleaseEvent) -> Result<(), WMError> {
        if event.detail == 1 {
            self.dragged_window = None;
        }

        Ok(())
    }

    fn handle_keypress_event(&mut self, event: KeyPressEvent) -> Result<(), WMError> {
        match event.detail {
            KEY_E => {
                if self.layout != Layout::Monocle {
                    self.prev_layout = self.layout;
                }

                self.layout = match self.layout {
                    Layout::Split => Layout::MasterStack,
                    Layout::MasterStack => Layout::Split,
                    Layout::Monocle => Layout::MasterStack,
                };

                self.should_relayout = true;
            }

            KEY_F => {
                if event
                    .state
                    .contains(KeyButMask::SHIFT | KeyButMask::CONTROL)
                {
                    let focused_window = self.conn.get_input_focus()?.reply()?.focus;
                    if focused_window == self.root {
                        return Ok(());
                    }

                    if let Some(index) = self.floating_clients.iter_mut().position(|c| {
                        c.window == focused_window || c.frame_window == focused_window
                    }) {
                        if self.clients.len() >= MAX_CLIENTS {
                            return Ok(());
                        }

                        let client = self.floating_clients.swap_remove(index);
                        self.clients.push(client);
                        self.should_relayout = true;

                        return Ok(());
                    }

                    self.clients.retain(|c| {
                        if c.frame_window == focused_window || c.window == focused_window {
                            self.floating_clients.push(*c);
                            assert!(self.floating_clients.len() <= MAX_CLIENTS);
                            return false;
                        }

                        true
                    });

                    self.should_relayout = true;

                    let Some(floating_client) = self.floating_clients.last_mut() else {
                        return Ok(());
                    };

                    let setup = self.conn.setup();
                    assert!(self.screen_num < setup.roots.len());
                    let screen = &setup.roots[self.screen_num];

                    let client_width = screen.width_in_pixels / 2;
                    let client_height = screen.height_in_pixels / 2;

                    floating_client.width = client_width;
                    floating_client.height = client_height;
                    floating_client.x = (client_width as i16) / 2;
                    floating_client.y = (client_height as i16) / 2;

                    self.conn.configure_window(
                        floating_client.frame_window,
                        &ConfigureWindowAux::new()
                            .stack_mode(StackMode::ABOVE)
                            .width(client_width as u32)
                            .height(client_height as u32)
                            .x(floating_client.x as i32)
                            .y(floating_client.y as i32),
                    )?;

                    self.conn.configure_window(
                        floating_client.window,
                        &ConfigureWindowAux::new()
                            .width(client_width as u32)
                            .height(client_height.saturating_sub(TITLEBAR_HEIGHT) as u32)
                            .x(0)
                            .y(TITLEBAR_HEIGHT as i32),
                    )?;
                    self.conn.set_input_focus(
                        InputFocus::PARENT,
                        floating_client.window,
                        event.time,
                    )?;

                    return Ok(());
                }

                self.layout = if self.layout == Layout::Monocle {
                    self.prev_layout
                } else {
                    self.prev_layout = self.layout;
                    Layout::Monocle
                };

                self.should_relayout = true;
            }

            KEY_ENTER => {
                std::process::Command::new("firefox")
                    .env("DISPLAY", &self.display_target)
                    .spawn()?;
            }

            KEY_Q => {
                std::process::Command::new("pkill").arg("Xephyr").output()?;
            }

            KEY_H => self.focus_direction(Direction::Left, event.time)?,
            KEY_L => self.focus_direction(Direction::Right, event.time)?,
            KEY_J => self.focus_direction(Direction::Down, event.time)?,
            KEY_K => self.focus_direction(Direction::Up, event.time)?,

            _ => {}
        }

        Ok(())
    }
}

/* ---------------------------------------------------------- */
/*                         FUNCTIONS                          */
/* ---------------------------------------------------------- */

fn center(client: &Client) -> (i16, i16) {
    (
        client.x.saturating_add(client.width as i16 / 2),
        client.y.saturating_add(client.height as i16 / 2),
    )
}

fn main() {
    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut wm = WM::new().unwrap();
    wm.scan_and_manage_windows().unwrap();
    // TODO(fcasibu): Make use of table for configuration
    wm.init_keymaps().unwrap();

    loop {
        if let Err(err) = wm.step() {
            tracing::error!(%err, "Error in WM loop");
            std::process::exit(1);
        }
    }
}

/* ---------------------------------------------------------- */
/*                      QUERIES/THOUGHTS                      */
/* ---------------------------------------------------------- */
