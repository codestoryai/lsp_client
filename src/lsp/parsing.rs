use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::Error;

use serde_json;

#[derive(Debug)]
pub enum ParseError {
    Io(Error),
    ParseInt(std::num::ParseIntError),
    Utf8(std::string::FromUtf8Error),
    Json(serde_json::Error),
    Unknown(String),
}

impl From<Error> for ParseError {
    fn from(err: Error) -> ParseError {
        ParseError::Io(err)
    }
}

impl From<std::string::FromUtf8Error> for ParseError {
    fn from(err: std::string::FromUtf8Error) -> ParseError {
        ParseError::Utf8(err)
    }
}

impl From<serde_json::Error> for ParseError {
    fn from(err: serde_json::Error) -> ParseError {
        ParseError::Json(err)
    }
}

impl From<std::num::ParseIntError> for ParseError {
    fn from(err: std::num::ParseIntError) -> ParseError {
        ParseError::ParseInt(err)
    }
}

impl From<String> for ParseError {
    fn from(s: String) -> ParseError {
        ParseError::Unknown(s)
    }
}

#[derive(Debug, PartialEq)]
/// A message header, as described in the Language Server Protocol specification.
enum LspHeader {
    ContentType,
    ContentLength(usize),
}

/// Given a reference to a reader, attempts to read a Language Server Protocol message,
/// blocking until a message is received.
pub async fn read_message<B: AsyncBufReadExt + Unpin>(
    reader: &mut B,
) -> Result<String, ParseError> {
    let mut buffer = String::new();
    let mut content_length: Option<usize> = None;

    // read in headers.
    loop {
        buffer.clear();
        reader.read_line(&mut buffer).await?;
        match &buffer {
            s if s.trim().len() == 0 => break, // empty line is end of headers
            s => {
                match parse_header(s)? {
                    LspHeader::ContentLength(len) => content_length = Some(len),
                    LspHeader::ContentType => (), // utf-8 only currently allowed value
                };
            }
        };
    }

    let content_length =
        content_length.ok_or(format!("missing content-length header: {}", buffer))?;
    // message body isn't newline terminated, so we read content_length bytes
    let mut body_buffer = vec![0; content_length];
    reader.read_exact(&mut body_buffer).await?;
    let body = String::from_utf8(body_buffer)?;
    Ok(body)
}

const HEADER_CONTENT_LENGTH: &'static str = "content-length";
const HEADER_CONTENT_TYPE: &'static str = "content-type";

/// Given a header string, attempts to extract and validate the name and value parts.
fn parse_header(s: &str) -> Result<LspHeader, ParseError> {
    let split: Vec<String> = s.split(": ").map(|s| s.trim().to_lowercase()).collect();
    if split.len() != 2 {
        return Err(ParseError::Unknown(format!("malformed header: {}", s)));
    }
    match split[0].as_ref() {
        HEADER_CONTENT_TYPE => Ok(LspHeader::ContentType),
        HEADER_CONTENT_LENGTH => Ok(LspHeader::ContentLength(usize::from_str_radix(
            &split[1], 10,
        )?)),
        _ => Err(ParseError::Unknown(format!("Unknown header: {}", s))),
    }
}
