// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A minimal IRC client for the general chat: it connects to Libera.Chat over TLS, joins one channel, and
//! passes on the messages of that channel and the private messages to this client. It runs on a thread of its
//! own; dropping the `IrcClient` quits.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

pub const SERVER: &str = "irc.libera.chat";
pub const PORT: u16 = 6697;
pub const CHANNEL: &str = "##openra-ingame-general";
/// `NICKLEN` of Libera.Chat.
pub const NICK_LEN: usize = 16;
pub const LONG_PREFIX: &str = "openra_user_";
pub const SHORT_PREFIX: &str = "ora_";
/// The longest message text that fits into one line of 512 bytes with the longest source prefix of Libera.Chat.
const MAX_TEXT_BYTES: usize = 400;

/// What the connection reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrcEvent {
    /// The channel is joined under this nick.
    Joined(String),
    /// A message in the channel, or a private message to this client.
    Message { nick: String, text: String, private: bool },
    /// The users of the channel changed; these are all of them, sorted.
    Users(Vec<String>),
    /// The connection is gone, with the reason.
    Disconnected(String),
}

enum Command {
    Send(String),
    Quit,
}

/// A connection on its own thread.
pub struct IrcClient {
    commands: Sender<Command>,
    events: Receiver<IrcEvent>,
    thread: Option<JoinHandle<()>>,
}

impl IrcClient {
    /// Connects with the nick for the player name `name`.
    pub fn connect(name: &str) -> IrcClient {
        let (command_tx, command_rx) = channel();
        let (event_tx, event_rx) = channel();
        let escaped = escape_name(name);
        let thread = std::thread::Builder::new()
            .name("irc".into())
            .spawn(move || {
                let reason = match run(&escaped, &command_rx, &event_tx) {
                    Ok(reason) => reason,
                    Err(e) => e.to_string(),
                };
                let _ = event_tx.send(IrcEvent::Disconnected(reason));
            })
            .ok();
        IrcClient {
            commands: command_tx,
            events: event_rx,
            thread,
        }
    }

    /// Sends `text` to the channel.
    pub fn send(&self, text: &str) {
        let _ = self.commands.send(Command::Send(text.to_string()));
    }

    /// The events since the last call.
    pub fn poll(&self) -> Vec<IrcEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for IrcClient {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                let _ = thread.join();
            });
        }
    }
}

/// The name with every character that IRC does not allow in nicks replaced by `_`. Letters, digits, `-` and
/// ``[]\`_^{|}`` are allowed.
pub fn escape_name(name: &str) -> String {
    let escaped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-[]\\`_^{|}".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if escaped.is_empty() { "player".into() } else { escaped }
}

/// The nick of attempt `attempt` (1, 2, 3, ...) for the escaped name: `openra_user_<name>`, or `ora_<name>` when
/// that is too long, with `_<attempt>` from the second attempt on. The name loses as many characters from its
/// end as necessary to fit `NICK_LEN`.
pub fn nick(escaped: &str, attempt: u32) -> String {
    let suffix = if attempt > 1 { format!("_{attempt}") } else { String::new() };
    if LONG_PREFIX.len() + escaped.len() + suffix.len() <= NICK_LEN {
        return format!("{LONG_PREFIX}{escaped}{suffix}");
    }
    let room = NICK_LEN.saturating_sub(SHORT_PREFIX.len() + suffix.len());
    let name: String = escaped.chars().take(room).collect();
    format!("{SHORT_PREFIX}{name}{suffix}")
}

/// The player name in a nick: the nick without `openra_user_` or `ora_`.
pub fn display_name(nick: &str) -> &str {
    nick.strip_prefix(LONG_PREFIX)
        .or_else(|| nick.strip_prefix(SHORT_PREFIX))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(nick)
}

/// One line from the server.
#[derive(Debug, PartialEq, Eq)]
struct Line<'a> {
    source_nick: Option<&'a str>,
    command: &'a str,
    params: Vec<&'a str>,
}

/// The users of the channel, from the `NAMES` replies and the joins, parts, quits, nick changes and kicks.
#[derive(Default)]
struct ChannelUsers {
    users: Vec<String>,
    /// The names of a `NAMES` reply that is not complete yet.
    reply: Option<Vec<String>>,
}

impl ChannelUsers {
    /// Applies the line; returns whether the users changed.
    fn apply(&mut self, line: &Line) -> bool {
        let in_channel = |channel: &str| channel.eq_ignore_ascii_case(CHANNEL);
        let source = line.source_nick.unwrap_or_default();
        match (line.command, line.params.as_slice()) {
            ("353", [_, _, channel, names]) if in_channel(channel) => {
                let reply = self.reply.get_or_insert_with(Vec::new);
                reply.extend(
                    names
                        .split(' ')
                        .map(|n| n.trim_start_matches(['~', '&', '@', '%', '+']))
                        .filter(|n| !n.is_empty())
                        .map(str::to_string),
                );
                false
            }
            ("366", [_, channel, ..]) if in_channel(channel) => match self.reply.take() {
                Some(names) => {
                    self.users = names;
                    true
                }
                None => false,
            },
            ("JOIN", [channel, ..]) if in_channel(channel) && self.position(source).is_none() => {
                self.users.push(source.to_string());
                true
            }
            ("PART", [channel, ..]) if in_channel(channel) => self.remove(source),
            ("QUIT", _) => self.remove(source),
            ("KICK", [channel, target, ..]) if in_channel(channel) => self.remove(target),
            ("NICK", [new_nick, ..]) => match self.position(source) {
                Some(i) => {
                    self.users[i] = new_nick.to_string();
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    fn position(&self, nick: &str) -> Option<usize> {
        self.users.iter().position(|u| u.eq_ignore_ascii_case(nick))
    }

    fn remove(&mut self, nick: &str) -> bool {
        self.position(nick).map(|i| self.users.remove(i)).is_some()
    }

    /// The users, sorted without regard to case.
    fn sorted(&self) -> Vec<String> {
        let mut users = self.users.clone();
        users.sort_by_key(|u| u.to_ascii_lowercase());
        users
    }
}

fn parse_line(line: &str) -> Option<Line<'_>> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    if rest.starts_with('@') {
        rest = rest.split_once(' ')?.1;
    }
    let mut source_nick = None;
    if let Some(stripped) = rest.strip_prefix(':') {
        let (source, after) = stripped.split_once(' ')?;
        source_nick = Some(source.split(['!', '@']).next().unwrap_or(source));
        rest = after;
    }
    let (command, mut rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let mut params = Vec::new();
    while !rest.is_empty() {
        if let Some(trailing) = rest.strip_prefix(':') {
            params.push(trailing);
            break;
        }
        let (param, after) = rest.split_once(' ').unwrap_or((rest, ""));
        if !param.is_empty() {
            params.push(param);
        }
        rest = after;
    }
    Some(Line {
        source_nick,
        command,
        params,
    })
}

/// The text without IRC formatting codes and other control characters. CTCP `ACTION` becomes `* text`; other
/// CTCP requests give `None`.
fn plain_text(text: &str) -> Option<String> {
    let text = match text.strip_prefix('\u{1}') {
        Some(ctcp) => format!("* {}", ctcp.trim_end_matches('\u{1}').strip_prefix("ACTION ")?),
        None => text.to_string(),
    };
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{3}' => {
                for _ in 0..2 {
                    chars.next_if(char::is_ascii_digit);
                }
                if chars.peek() == Some(&',') {
                    let mut lookahead = chars.clone();
                    lookahead.next();
                    if lookahead.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                        for _ in 0..2 {
                            chars.next_if(char::is_ascii_digit);
                        }
                    }
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    Some(out)
}

/// `text` cut to `MAX_TEXT_BYTES`, at a character boundary, without line breaks.
fn outgoing_text(text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

type TlsStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

fn open() -> std::io::Result<TlsStream> {
    let address = (SERVER, PORT)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, "no address for the IRC server"))?;
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(15))?;
    socket.set_read_timeout(Some(Duration::from_secs(15)))?;
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(SERVER).map_err(std::io::Error::other)?;
    let mut connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(std::io::Error::other)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut socket)?;
    }
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(rustls::StreamOwned::new(connection, socket))
}

/// Runs the connection until it ends; returns the reason. Reads time out after 100 ms, so that the commands are
/// checked between them; the TLS handshake is done before, because a timeout would break it.
fn run(escaped: &str, commands: &Receiver<Command>, events: &Sender<IrcEvent>) -> std::io::Result<String> {
    let mut stream = BufReader::new(open()?);
    let mut attempt = 1;
    let mut own_nick = nick(escaped, attempt);
    let mut joined = false;
    let mut users = ChannelUsers::default();
    let write = |stream: &mut BufReader<TlsStream>, line: &str| -> std::io::Result<()> {
        let s = stream.get_mut();
        s.write_all(line.as_bytes())?;
        s.write_all(b"\r\n")?;
        s.flush()
    };
    write(&mut stream, &format!("NICK {own_nick}"))?;
    write(&mut stream, "USER rustora 0 * :rustora general chat")?;
    let mut buffer = Vec::new();
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Send(text)) if joined => {
                    write(&mut stream, &format!("PRIVMSG {CHANNEL} :{}", outgoing_text(&text)))?;
                }
                Ok(Command::Send(_)) => {}
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => {
                    let _ = write(&mut stream, "QUIT :leaving");
                    return Ok("quit".into());
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        match stream.read_until(b'\n', &mut buffer) {
            Ok(0) => return Ok("connection closed".into()),
            Ok(_) if buffer.ends_with(b"\n") => {}
            Ok(_) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                continue;
            }
            Err(e) => return Err(e),
        }
        let text = String::from_utf8_lossy(&buffer).into_owned();
        buffer.clear();
        let Some(line) = parse_line(&text) else { continue };
        if users.apply(&line) {
            let _ = events.send(IrcEvent::Users(users.sorted()));
        }
        match (line.command, line.params.as_slice()) {
            ("PING", params) => write(&mut stream, &format!("PONG :{}", params.first().unwrap_or(&"")))?,
            ("001", [nick, ..]) => {
                own_nick = nick.to_string();
                write(&mut stream, &format!("JOIN {CHANNEL}"))?;
            }
            ("432" | "433" | "436", _) if !joined => {
                attempt += 1;
                own_nick = nick(escaped, attempt);
                write(&mut stream, &format!("NICK {own_nick}"))?;
            }
            ("JOIN", [channel, ..]) if line.source_nick == Some(own_nick.as_str()) && channel.eq_ignore_ascii_case(CHANNEL) => {
                joined = true;
                let _ = events.send(IrcEvent::Joined(own_nick.clone()));
            }
            ("NICK", [new_nick, ..]) if line.source_nick == Some(own_nick.as_str()) => own_nick = new_nick.to_string(),
            ("PRIVMSG", [target, message]) => {
                let private = target.eq_ignore_ascii_case(&own_nick);
                if !private && !target.eq_ignore_ascii_case(CHANNEL) {
                    continue;
                }
                if let (Some(nick), Some(text)) = (line.source_nick, plain_text(message)) {
                    let _ = events.send(IrcEvent::Message {
                        nick: nick.to_string(),
                        text,
                        private,
                    });
                }
            }
            ("ERROR", params) => return Ok(params.last().unwrap_or(&"error").to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nicks() {
        assert_eq!(nick("Bob", 1), "openra_user_Bob");
        assert_eq!(nick("Bob", 2), "ora_Bob_2");
        assert_eq!(nick("Abc", 2), "ora_Abc_2");
        assert_eq!(nick("Ab", 2), "openra_user_Ab_2");
        assert_eq!(nick("Bobby", 1), "ora_Bobby");
        assert_eq!(nick("Commander", 1), "ora_Commander");
        assert_eq!(nick("Commander", 2), "ora_Commander_2");
        assert_eq!(nick("Commander", 10), "ora_Commander_10");
        assert_eq!(nick("AVeryLongPlayerName", 1), "ora_AVeryLongPla");
        assert_eq!(nick("AVeryLongPlayerName", 4), "ora_AVeryLongP_4");
        assert_eq!(nick("AVeryLongPlayerName", 12), "ora_AVeryLong_12");
        assert_eq!(escape_name("Mr. Müller"), "Mr__M_ller");
        assert_eq!(escape_name(""), "player");
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("openra_user_Bob"), "Bob");
        assert_eq!(display_name("ora_Commander_2"), "Commander_2");
        assert_eq!(display_name("someone"), "someone");
        assert_eq!(display_name("ora_"), "ora_");
    }

    #[test]
    fn lines() {
        let line = parse_line(":ora_x!~u@host PRIVMSG ##openra-ingame-general :hello there\r\n").unwrap();
        assert_eq!(line.source_nick, Some("ora_x"));
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params, vec!["##openra-ingame-general", "hello there"]);
        let line = parse_line("PING :tantalum.libera.chat").unwrap();
        assert_eq!(
            (line.source_nick, line.command, line.params),
            (None, "PING", vec!["tantalum.libera.chat"])
        );
        let line = parse_line("@time=x :srv 433 * openra_user_Bob :Nickname is already in use").unwrap();
        assert_eq!(line.command, "433");
    }

    #[test]
    fn channel_users() {
        let mut users = ChannelUsers::default();
        let mut apply = |text: &str| users.apply(&parse_line(text).unwrap());
        assert!(!apply(":srv 353 me = ##openra-ingame-general :me @op +voiced"));
        assert!(!apply(":srv 353 me = ##openra-ingame-general :Zed"));
        assert!(apply(":srv 366 me ##openra-ingame-general :End of /NAMES list."));
        assert!(apply(":ora_Bob!u@h JOIN ##openra-ingame-general"));
        assert!(!apply(":x!u@h JOIN #other"));
        assert!(apply(":op!u@h NICK :op2"));
        assert!(apply(":voiced!u@h PART ##openra-ingame-general :bye"));
        assert!(apply(":Zed!u@h QUIT :gone"));
        assert!(!apply(":nobody!u@h QUIT :gone"));
        assert!(apply(":op2!u@h KICK ##openra-ingame-general ora_bob :out"));
        assert_eq!(users.sorted(), ["me", "op2"]);
    }

    #[test]
    fn texts() {
        assert_eq!(plain_text("\u{2}bold\u{2} \u{3}04,12red\u{3} end").unwrap(), "bold red end");
        assert_eq!(plain_text("\u{1}ACTION waves\u{1}").unwrap(), "* waves");
        assert_eq!(plain_text("\u{1}VERSION\u{1}"), None);
        assert_eq!(outgoing_text("a\nb"), "ab");
        assert_eq!(outgoing_text(&"ä".repeat(300)).len(), 400);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    fn wait_joined(client: &IrcClient) -> String {
        for _ in 0..300 {
            for event in client.poll() {
                match event {
                    IrcEvent::Joined(nick) => return nick,
                    IrcEvent::Disconnected(reason) => panic!("disconnected: {reason}"),
                    IrcEvent::Message { .. } | IrcEvent::Users(_) => {}
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("not joined");
    }

    /// Joins Libera.Chat twice under the same name; the second gets `_2`.
    #[test]
    #[ignore = "connects to Libera.Chat"]
    fn join_twice() {
        let first = IrcClient::connect("rtst");
        assert_eq!(wait_joined(&first), "openra_user_rtst");
        let second = IrcClient::connect("rtst");
        assert_eq!(wait_joined(&second), "ora_rtst_2");
    }

    /// Waits for a private message; a second IRC client has to send one to `openra_user_rpm`.
    #[test]
    #[ignore = "connects to Libera.Chat"]
    fn private_message() {
        let client = IrcClient::connect("rpm");
        wait_joined(&client);
        for _ in 0..900 {
            for event in client.poll() {
                if let IrcEvent::Message { nick, text, private } = event {
                    eprintln!("{nick}: {text} {private}");
                    assert!(private);
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("no message");
    }
}
