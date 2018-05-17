extern crate rand;

#[macro_use]
extern crate actix;
extern crate actix_web;

use actix::*;
use actix_web::*;
use actix_web::server::HttpServer;

use std::time::Instant;

mod server;

mod client {
    /// Send message to specific room
    #[derive(Message)]
    pub struct Message {
        /// Id of the client session
        pub id: usize,
        /// Peer message
        pub msg: String,
    }
}

/// This is our WebSocket route state, this state is shared with all route instances
/// via `HttpContext::state()`
struct WsChatSessionState {
    addr: Addr<Syn, server::ChatServer>,
}

/// Entry point for our route
fn chat_route(req: HttpRequest<WsChatSessionState>) -> Result<HttpResponse> {
    ws::start(
        req,
        WsChatSession {
            id: 0,
            hb: Instant::now(),
            name: None,
        },
    )
}

struct WsChatSession {
    /// unique session id
    id: usize,
    /// Client must send ping at least once per 10 seconds, otherwise we drop connection.
    hb: Instant,
    /// peer name
    name: Option<String>,
}

impl Actor for WsChatSession {
    type Context = ws::WebsocketContext<Self, WsChatSessionState>;

    /// Method is called on actor start.
    /// We register ws session with ChatServer
    fn started(&mut self, ctx: &mut Self::Context) {
        // register self in chat server. `AsyncContext::wait` register
        // future within context, but context waits until this future resolves
        // before processing any other events.
        // HttpContext::state() is instance of WsChatSessionState, state is shared across all
        // routes within application
        let addr: Addr<Syn, _> = ctx.address();
        ctx.state()
            .addr
            .send(server::Connect {
                addr: addr.recipient(),
            })
            .into_actor(self)
            .then(|res, act, ctx| {
                match res {
                    Ok(res) => act.id = res,
                    // something is wrong with chat server
                    _ => ctx.stop(),
                }
                fut::ok(())
            })
            .wait(ctx);
    }

    fn stopping(&mut self, ctx: &mut Self::Context) -> Running {
        // notify chat server
        ctx.state().addr.do_send(server::Disconnect { id: self.id });
        Running::Stop
    }
}

/// Handle messages from chat server, we simply send it to peer WebSocket
impl Handler<server::Message> for WsChatSession {
    type Result = ();

    fn handle(&mut self, msg: server::Message, ctx: &mut Self::Context) {
        ctx.text(msg.0);
    }
}

/// WebSocket message handler
impl StreamHandler<ws::Message, ws::ProtocolError> for WsChatSession {
    fn handle(&mut self, msg: ws::Message, ctx: &mut Self::Context) {
        println!("WebSocket message: {:?}", msg);
        match msg {
            ws::Message::Ping(msg) => ctx.pong(&msg),
            ws::Message::Pong(_) => self.hb = Instant::now(),
            ws::Message::Text(text) => {
                let m = text.trim();
                // we check for /sss type of messages
                if m.starts_with('/') {
                    let v: Vec<&str> = m.splitn(2, ' ').collect();
                    match v[0] {
                        "/list" => {
                            // Send ListRooms message to chat server and wait for response
                            println!("List rooms");
                            ctx.state()
                                .addr
                                .send(server::ListRooms)
                                .into_actor(self)
                                .then(|res, _, ctx| {
                                    match res {
                                        Ok(rooms) => for room in rooms {
                                            ctx.text(room);
                                        },
                                        _ => println!("Something is wrong"),
                                    }
                                    fut::ok(())
                                })
                                .wait(ctx)
                            // .wait(ctx) pauses all events in context,
                            // so actor wont receive any new messages until it get list
                            // of rooms back
                        }
                        "/join" => {
                            if v.len() == 2 {
                                ctx.state().addr.do_send(server::Join { id: self.id });

                                ctx.text("joined");
                            } else {
                                ctx.text("!!! room name is required");
                            }
                        }
                        "/name" => {
                            if v.len() == 2 {
                                self.name = Some(v[1].to_owned());
                            } else {
                                ctx.text("!!! name is required");
                            }
                        }
                        _ => ctx.text(format!("!!! unknown command: {:?}", m)),
                    }
                } else {
                    let msg = if let Some(ref name) = self.name {
                        format!("{}: {}", name, m)
                    } else {
                        m.to_owned()
                    };
                    // send message to chat server
                    ctx.state()
                        .addr
                        .do_send(client::Message { id: self.id, msg })
                }
            }
            ws::Message::Binary(_) => println!("Unexpected binary"),
            ws::Message::Close(_) => {
                ctx.stop();
            }
        }
    }
}

fn main() {
    let sys = actix::System::new("ws-chat");

    // Start chat server actor in separate thread
    let server: Addr<Syn, _> = Arbiter::start(|_| server::ChatServer::default());

    // Create Http server with WebSocket support
    HttpServer::new(move || {
        // WebSocket sessions state
        let state = WsChatSessionState {
            addr: server.clone(),
        };

        App::with_state(state)
                .resource("/ws/", |r| r.f(chat_route))
                // static resources
                .handler("/", fs::StaticFiles::new("static/").index_file("index.html"))
    }).bind("localhost:8080")
        .unwrap()
        .start();

    println!("Started http server: http://localhost:8080");
    let _ = sys.run();
}
