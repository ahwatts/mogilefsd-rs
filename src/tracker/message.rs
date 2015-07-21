use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::str;
use super::super::error::{MogError, MogResult};
use url::{form_urlencoded, percent_encoding};

/// The different commands that the tracker implements.
#[derive(Debug)]
pub enum Command {
    CreateOpen,
    Noop,
}

impl Command {
    pub fn from_optional_bytes(bytes: Option<&[u8]>) -> MogResult<Command> {
        use self::Command::*;

        match bytes.map(|bs| str::from_utf8(bs)) {
            Some(Ok(string)) if string == "create_open" => Ok(CreateOpen),
            Some(Ok(string)) if string == "noop" => Ok(Noop),
            Some(Ok(string)) => Err(MogError::UnknownCommand(Some(string.to_string()))),
            Some(Err(utf8e)) => Err(MogError::Utf8(utf8e)),
            None => Err(MogError::UnknownCommand(None)),
        }
    }
}

impl Display for Command {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        use self::Command::*;

        let op_str = match *self {
            CreateOpen => "create_open",
            Noop => "noop",
        };

        write!(f, "{}", op_str)
    }
}

/// A request to the MogileFS tracker.
#[derive(Debug)]
pub struct Request {
    pub op: Command,
    pub args: Vec<(String, String)>,
}

impl Request {
    pub fn from_bytes(bytes: &[u8]) -> MogResult<Request> {
        let mut toks = bytes.split(|&c| c == b' ');
        let command = try!(Command::from_optional_bytes(toks.next()));

        Ok(Request {
            op: command,
            args: form_urlencoded::parse(toks.next().unwrap_or(b"")),
        })
    }

    pub fn args_hash<'a>(&'a self) -> HashMap<&'a str, &'a str> {
        self.args.iter().fold(HashMap::new(), |mut m, &(ref k, ref v)| {
            *m.entry(k).or_insert(v) = v; m
        })
    }
}

/// The response from the tracker.
#[derive(Debug)]
pub struct Response(MogResult<Vec<(String, String)>>);

impl Response {
    pub fn render(&self) -> Vec<u8> {
        match self.0 {
            Ok(ref args) => format!("OK {}\r\n", form_urlencoded::serialize(args)).into_bytes(),
            Err(ref err) => {
                let encoded_description = percent_encoding::percent_encode(
                    format!("{}", err).as_bytes(),
                    percent_encoding::FORM_URLENCODED_ENCODE_SET);
                format!("ERR {} {}\r\n", err.error_kind(), encoded_description).into_bytes()
            }
        }
    }
}

impl From<MogResult<Response>> for Response {
    fn from(result: MogResult<Response>) -> Response {
        match result {
            Ok(response) => response,
            Err(err) => Response(Err(err)),
        }
    }
}
