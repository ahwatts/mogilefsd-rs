use super::Tracker;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::thread;
use std::io::{self, Write, BufRead, BufReader};
use std::sync::Arc;

pub struct ThreadedListener {
    listener: TcpListener,
    tracker: Arc<Tracker>,
}

impl ThreadedListener {
    pub fn new<S: ToSocketAddrs>(addr: S, tracker: Tracker) -> Result<ThreadedListener, io::Error> {
        Ok(ThreadedListener {
            listener: try!(TcpListener::bind(addr)),
            tracker: Arc::new(tracker),
        })
    }

    pub fn run(&self) {
        for stream in self.listener.incoming() {
            match stream {
                Ok(stream) => {
                    let conn_tracker = self.tracker.clone();
                    thread::spawn(move|| handle_connection(stream, conn_tracker));
                },
                Err(e) => {
                    error!("Connection failed: {}", e);
                }
            }
        }
    }
}

fn handle_connection(mut writer: TcpStream, tracker: Arc<Tracker>) -> Result<(), io::Error> {
    let reader = BufReader::new(try!(writer.try_clone()));

    for line in reader.split(b'\n') {
        let mut line = try!(line);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let response = tracker.handle(line.as_ref());
        try!(writer.write_all(response.render().as_bytes()));
    }

    Ok(())
}
