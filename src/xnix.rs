include!("utils.rs");

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::prelude::{CommandExt, FromRawFd};
use nix::libc::{self, STDIN_FILENO, STDOUT_FILENO};
use nix::pty::openpty;
use nix::sys::{termios};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::{thread};
use websocket::sync::{Server, Writer};
use websocket::{ClientBuilder, OwnedMessage};
use atty::Stream;
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;
use ioctl_rs;
use nix::unistd::{fork, ForkResult};
use std::process::{Command, Stdio};

pub fn get_termsize(fd : i32) -> Option<Box<libc::winsize>> {
	let mut ret = 0;
	let mut size = Box::new(libc::winsize{
		ws_row : 25 , 
		ws_col : 80 ,
		ws_xpixel : 0,
		ws_ypixel: 0, 
		
	});

	if atty::is(Stream::Stdin){
		ret = unsafe {libc::ioctl(fd , libc::TIOCGWINSZ , &mut *size) } as i32;
	} else {
		size.ws_row = 25;
		size.ws_col = 80;
	};

	if ret < 0 {
		return None;
	}

	Some(size)
}

pub fn set_termsize(fd : i32 , mut size : Box<libc::winsize>) -> bool {
	(unsafe {libc::ioctl(fd , libc::TIOCSWINSZ , &mut *size) } as i32) > 0
}

pub fn rconnect( addr : String , subprocess : String , fullargs : Vec<String>){

	let client = match  { 
		match ClientBuilder::new(addr.as_str()){
			Err(_) => {
				log::error!("parse address [{}] faild. eg : ws://127.0.0.1:8000" , addr);
				return;
			},
			Ok(p) => p
		}
	}.connect_insecure() {
		Err(_) => {
			log::error!("connect [{}] faild" , addr);
			return;
		},
		Ok(p) => p
	};

	let (mut receiver, mut sender) = client.split().unwrap();
	let (tx, rx) = channel();

	let tx_1 = tx.clone();

	let ends = openpty(None, None).expect("openpty failed");
	let master = ends.master;
	let slave = ends.slave;

	let mut builder = Command::new(subprocess.clone());

	if fullargs.len() !=  0 {
		builder.args(fullargs);
	} 

	log::info!("start process: [{}]" ,subprocess );
	match unsafe { fork() } {
		Ok(ForkResult::Parent { child: pid, .. }) => {
			thread::spawn(move || {
				let mut status = 0;
				unsafe { libc::waitpid(i32::from(pid), &mut status ,0) };
				log::warn!("child process exit!");
				std::process::exit(0);
			});

		}
		Ok(ForkResult::Child) => {
			unsafe { ioctl_rs::ioctl(master, ioctl_rs::TIOCNOTTY) };
			unsafe { libc::setsid() };
			unsafe { ioctl_rs::ioctl(slave, ioctl_rs::TIOCSCTTY) };

			builder
			.stdin(unsafe { Stdio::from_raw_fd(slave) })
			.stdout(unsafe { Stdio::from_raw_fd(slave) })
			.stderr(unsafe { Stdio::from_raw_fd(slave) })
			.exec();
			return;
		},
		Err(_) => println!("Fork failed"),
	}

	let ptyin = unsafe { File::from_raw_fd(master) };
	let mut ptyout = unsafe { File::from_raw_fd(master) };
	
	let rc_writer = Arc::new(Mutex::new(ptyin));

	thread::spawn(move || {

		let mut buf : [u8;1024] = [0;1024];
		loop {

			let result = ptyout.read(buf.as_mut());
			let size = result.unwrap();	

			
			if size == 0 {
				break;
			}

			let msg = OwnedMessage::Binary(buf.to_vec());
			match tx.send(msg) {
				Ok(()) => (),
				Err(_) => {
					break;
				}
			}
			buf.fill(0);
		}

	});

	let send_loop = thread::spawn(move || {
		loop {
			let message = match rx.recv() {
				Ok(m) => m,
				Err(_) => {
					return;
				}
			};
			match message {
				OwnedMessage::Close(_) => {
					std::process::exit(0);
				},
				OwnedMessage::Binary(_) => {
					let _ = sender.send_message(&message);
				},
				OwnedMessage::Text(_) => {
					let _ = sender.send_message(&message);
				},
				OwnedMessage::Ping(message) => {
					let _ = sender.send_message(&OwnedMessage::Ping(message));
				},
				OwnedMessage::Pong(_) => {},

			}
		}
	});

	let receive_loop = thread::spawn(move || {

		for message in receiver.incoming_messages() {
			let message = match message {
				Ok(m) => m,
				Err(_) => {
					let _ = tx_1.send(OwnedMessage::Close(None));
					return;
				}
			};
			match message {
				OwnedMessage::Close(_) => {
					let _ = tx_1.send(OwnedMessage::Close(None));
					return;
				},
				OwnedMessage::Ping(message) => {
					let _ = tx_1.send(OwnedMessage::Pong(message));
				},
				OwnedMessage::Text(text) => {
					let mut writer = rc_writer.lock().unwrap();
					writer.write_all(text.as_bytes()).unwrap();
					
				},
				OwnedMessage::Binary(data) => {

					if data.len() == 6{

						if data[0] == MAGIC_FLAG[0] && data[1] == MAGIC_FLAG[1] {

							let size = Box::new(libc::winsize{
								ws_row : makeword(data[2] , data[3]), 
								ws_col :  makeword(data[4] , data[5]) ,
								ws_xpixel : 0,
								ws_ypixel: 0, 
								
							});
							
							if set_termsize(slave , size) {
								std::process::exit(0);
							}

							continue;
						}
					}


					let mut writer = rc_writer.lock().unwrap();
					writer.write_all(data.as_slice()).unwrap();
				},
				OwnedMessage::Pong(_) => {
					//let _ = tx_1.send(OwnedMessage::Ping([0].to_vec()));
				},
			}
		}
	});

	let _ = send_loop.join();
	let _ = receive_loop.join();

	return;
}

pub fn rbind(port : String){

	let bakflag = termios::tcgetattr(STDOUT_FILENO).unwrap();

	log::info!("listen to: [{}:{}]" ,"0.0.0.0" , port );
	let listen_addr = format!("{}:{}", "0.0.0.0", port);

	let mut server = match Server::bind(listen_addr) {
		Err(_) => {
			log::error!("bind [0.0.0.0:{}] faild" , port);
			return;
		}, 
		Ok(p) => p
	};

	let request = server.accept().unwrap();
	let client = request.accept().unwrap();

	let port = client.peer_addr().unwrap().port();
	let ip = client.peer_addr().unwrap().ip();

	log::info!("accept from : [{}:{}]" ,ip , port );

	let (mut receiver, sender) = client.split().unwrap();
	

	let slck = Arc::new(Mutex::new(sender));
	let slck_1 = slck.clone();
	let slck_2 = slck.clone();
	
	if atty::is(Stream::Stdin) {

		let mut flags = termios::tcgetattr(STDIN_FILENO).unwrap();

		flags.input_flags |= termios::InputFlags::IGNPAR;
		flags.input_flags &= !{termios::InputFlags::ISTRIP|termios::InputFlags::INLCR|termios::InputFlags::IGNCR|termios::InputFlags::ICRNL|termios::InputFlags::IXON|termios::InputFlags::IXANY|termios::InputFlags::IXOFF};
		flags.local_flags &= !{termios::LocalFlags::ISIG|termios::LocalFlags::ICANON|termios::LocalFlags::ECHO|termios::LocalFlags::ECHOE|termios::LocalFlags::ECHOK|termios::LocalFlags::ECHONL|termios::LocalFlags::IEXTEN};
		flags.output_flags &= !termios::OutputFlags::OPOST;
		flags.control_chars[nix::libc::VMIN] = 1;
		flags.control_chars[nix::libc::VTIME] = 0;

		termios::tcsetattr(STDIN_FILENO, termios::SetArg::TCSANOW, &flags).unwrap();
	}

	let mut signals = Signals::new(&[SIGWINCH]).unwrap();

	thread::spawn(move || {

		for sig in signals.forever() {

			if sig == SIGWINCH {

				let size = get_termsize(0).unwrap();

				let (ws_row1 ,ws_row2) = splitword(size.ws_row);
				let (ws_col1 ,ws_col2) = splitword(size.ws_col);

				let vec = [MAGIC_FLAG[0], MAGIC_FLAG[1] , ws_row1 ,ws_row2 , ws_col1 ,ws_col2 ];

				let msg = OwnedMessage::Binary(vec.to_vec());
				slck.lock().unwrap().send_message(&msg).unwrap();
			}
		}
	});

	thread::spawn(move || {
		// first set terminal size
		let size = get_termsize(0).unwrap();
		let (ws_row1 ,ws_row2) = splitword(size.ws_row);
		let (ws_col1 ,ws_col2) = splitword(size.ws_col);

		let vec = [MAGIC_FLAG[0], MAGIC_FLAG[1] , ws_row1 ,ws_row2 , ws_col1 ,ws_col2 ];
		let msg = OwnedMessage::Binary(vec.to_vec());
		{
			slck_1.lock().unwrap().send_message(&msg).unwrap();
		}

		let mut fin = unsafe {File::from_raw_fd(0)};

		loop{
			
			let mut buf : [u8;1] = [0];
			let size = fin.read(buf.as_mut()).unwrap();

			if size == 0 {
				break;
			}

			let msg = OwnedMessage::Binary(buf.to_vec());
			slck_1.lock().unwrap().send_message(&msg).unwrap();
		}
	});


	let mut out = unsafe {File::from_raw_fd(1)};

	for message in receiver.incoming_messages() {
		let message = match message {
			Ok(p) => p,
			Err(_) => {
				termios::tcsetattr(STDIN_FILENO, termios::SetArg::TCSANOW, &bakflag).unwrap();
				log::warn!("client closed : [{}:{}]" ,ip , port );
				std::process::exit(0);
			},
		};
		
		match message {
			OwnedMessage::Close(_) => {
				termios::tcsetattr(STDIN_FILENO, termios::SetArg::TCSANOW, &bakflag).unwrap();
				log::warn!("client closed : [{}:{}]" ,ip , port );
				std::process::exit(0);
			},
			OwnedMessage::Ping(ping) => {
				let message = OwnedMessage::Pong(ping);
				slck_2.lock().unwrap().send_message(&message).unwrap();
			},
			OwnedMessage::Text(text) => {
				out.write_all(text.as_bytes()).unwrap();
				
			},
			OwnedMessage::Binary(data) => {
				out.write_all(data.as_slice()).unwrap();
			},
			_ => {},
		}
	}
}

pub fn connect( addr : String ){

	let bakflag = termios::tcgetattr(STDOUT_FILENO).unwrap();

	let client = match  { 
		match ClientBuilder::new(addr.as_str()){
			Err(_) => {
				log::error!("parse address [{}] faild. eg : ws://127.0.0.1:8000" , addr);
				return;
			},
			Ok(p) => p
		}
	}.connect_insecure(){
		Err(_) => {
			log::error!("connect [{}] faild" , addr);
			return;
		},
		Ok(p) => p
	};

	let (mut receiver, mut sender) = client.split().unwrap();

	let (tx, rx) = channel();

	let tx_1 = tx.clone();
	let tx_2 = tx.clone();
	let send_loop = thread::spawn(move || {
		loop {
			let message = match rx.recv() {
				Ok(m) => m,
				Err(_) => {
					return;
				}
			};
			match message {
				OwnedMessage::Close(_) => {
					termios::tcsetattr(STDIN_FILENO, termios::SetArg::TCSANOW, &bakflag).unwrap();
					std::process::exit(0);
				},
				OwnedMessage::Binary(_) => {
					let _ = sender.send_message(&message);
				},
				OwnedMessage::Text(_) => {
					let _ = sender.send_message(&message);
				},
				OwnedMessage::Ping(message) => {
					let _ = sender.send_message(&OwnedMessage::Ping(message));
				},
				OwnedMessage::Pong(_) => {},

			}
		}
	});

	let receive_loop = thread::spawn(move || {

		let mut out = unsafe {File::from_raw_fd(1)};

		for message in receiver.incoming_messages() {
			let message = match message {
				Ok(m) => m,
				Err(_) => {
					let _ = tx_1.send(OwnedMessage::Close(None));
					return;
				}
			};
			match message {
				OwnedMessage::Close(_) => {
					let _ = tx_1.send(OwnedMessage::Close(None));
					return;
				},
				OwnedMessage::Ping(message) => {
					let _ = tx_1.send(OwnedMessage::Pong(message));
				},
				OwnedMessage::Text(message) => {
					out.write_all(message.as_bytes()).unwrap();
				},
				OwnedMessage::Binary(message) => {
					out.write_all(message.as_slice()).unwrap();
				},
				OwnedMessage::Pong(_) => {
					//let _ = tx_1.send(OwnedMessage::Ping([0].to_vec()));
				},
			}
		}
	});

	let mut signals = Signals::new(&[SIGWINCH]).unwrap();

	thread::spawn(move || {

		for sig in signals.forever() {

			if sig == SIGWINCH {

				let size = get_termsize(0).unwrap();

				let (ws_row1 ,ws_row2) = splitword(size.ws_row);
				let (ws_col1 ,ws_col2) = splitword(size.ws_col);

				let vec = [MAGIC_FLAG[0], MAGIC_FLAG[1] , ws_row1 ,ws_row2 , ws_col1 ,ws_col2 ];

				let msg = OwnedMessage::Binary(vec.to_vec());
				match tx_2.send(msg) {
					Ok(()) => (),
					Err(_) => {
						break;
					}
				}
			}
		}
	});

	if atty::is(Stream::Stdin) {

		let mut flags = termios::tcgetattr(STDIN_FILENO).unwrap();

		flags.input_flags |= termios::InputFlags::IGNPAR;
		flags.input_flags &= !{termios::InputFlags::ISTRIP|termios::InputFlags::INLCR|termios::InputFlags::IGNCR|termios::InputFlags::ICRNL|termios::InputFlags::IXON|termios::InputFlags::IXANY|termios::InputFlags::IXOFF};
		flags.local_flags &= !{termios::LocalFlags::ISIG|termios::LocalFlags::ICANON|termios::LocalFlags::ECHO|termios::LocalFlags::ECHOE|termios::LocalFlags::ECHOK|termios::LocalFlags::ECHONL|termios::LocalFlags::IEXTEN};
		flags.output_flags &= !termios::OutputFlags::OPOST;
		flags.control_chars[nix::libc::VMIN] = 1;
		flags.control_chars[nix::libc::VTIME] = 0;

		termios::tcsetattr(STDIN_FILENO, termios::SetArg::TCSANOW, &flags).unwrap();
	}
	
	// first set terminal size
	let size = get_termsize(0).unwrap();
	let (ws_row1 ,ws_row2) = splitword(size.ws_row);
	let (ws_col1 ,ws_col2) = splitword(size.ws_col);

	let vec = [MAGIC_FLAG[0], MAGIC_FLAG[1] , ws_row1 ,ws_row2 , ws_col1 ,ws_col2 ];
	let msg = OwnedMessage::Binary(vec.to_vec());
	tx.send(msg).unwrap();

	let mut fin = unsafe {File::from_raw_fd(0)};

	loop{
		
		let mut buf : [u8;1] = [0];
		let size = fin.read(buf.as_mut()).unwrap();

		if size == 0 {
			break;
		}

		let msg = OwnedMessage::Binary(buf.to_vec());
		match tx.send(msg) {
			Ok(()) => (),
			Err(_) => {
				break;
			}
		}
	}

	let _ = send_loop.join();
	let _ = receive_loop.join();

	return;
}

pub fn bind(port : String , subprocess : String , fullargs : Vec<String>) {

	let ends = openpty(None, None).expect("openpty failed");
	let master = ends.master;
	let slave = ends.slave;

	let mut builder = Command::new(subprocess.clone());

	if fullargs.len() !=  0 {
		builder.args(fullargs);
	} 

	log::info!("start process: [{}]" ,subprocess );
	match unsafe { fork() } {
		Ok(ForkResult::Parent { child: pid, .. }) => {
			thread::spawn(move || {
				let mut status = 0;
				unsafe { libc::waitpid(i32::from(pid), &mut status ,0) };
				log::warn!("child process exit!");
				std::process::exit(0);
			});

		}
		Ok(ForkResult::Child) => {
			unsafe { ioctl_rs::ioctl(master, ioctl_rs::TIOCNOTTY) };
			unsafe { libc::setsid() };
			unsafe { ioctl_rs::ioctl(slave, ioctl_rs::TIOCSCTTY) };

			builder
			.stdin(unsafe { Stdio::from_raw_fd(slave) })
			.stdout(unsafe { Stdio::from_raw_fd(slave) })
			.stderr(unsafe { Stdio::from_raw_fd(slave) })
			.exec();
			return;
		},
		Err(_) => println!("Fork failed"),
	}

	let mut ptyin = unsafe { File::from_raw_fd(master) };
	let mut ptyout = unsafe { File::from_raw_fd(master) };

	let history : Vec<u8> = Vec::new();
	let history_lck1 = Arc::new(Mutex::new(history)); 

	let senders : HashMap<u16 , Arc<Mutex<Writer<std::net::TcpStream>>>> = HashMap::new();

	let sender_lck1 = Arc::new(Mutex::new(senders));

	let sender_lck2 = sender_lck1.clone();
	let history_lck2 = history_lck1.clone();
	thread::spawn(move || {

		let mut buf : [u8;1024] = [0;1024];
		loop {

			let result = ptyout.read(buf.as_mut());
			let size = result.unwrap();	

			if size == 0{
				std::process::exit(0);
			}

			{ history_lck2.lock().unwrap().append(buf[..size].to_vec().as_mut()); }
			
			let mut map = sender_lck2.lock().unwrap();
			for i in map.iter_mut(){
				let msg = OwnedMessage::Binary(buf[..size].to_vec());
				match i.1.lock().unwrap().send_message(&msg){
					Ok(p) => p ,
					Err(e) => {
						println!("{}",e);
					}
				};
			}
			buf.fill(0);
		}

	});

	log::info!("listen to: [{}:{}]" ,"0.0.0.0" , port );
	let listen_addr = format!("{}:{}", "0.0.0.0", port);

	let mut server = match Server::bind(listen_addr) {
		Err(_) => {
			log::error!("bind [0.0.0.0:{}] faild" , port);
			return;
		}, 
		Ok(p) => p
	};

	let request = server.accept().unwrap();
	
	let client = request.accept().unwrap();
	let port = client.peer_addr().unwrap().port();
	let ip = client.peer_addr().unwrap().ip();

	log::info!("accept from : [{}:{}]" ,ip , port );

	let (mut receiver, mut sender) = client.split().unwrap();
	{
		let data = history_lck1.lock().unwrap();
		let msg =OwnedMessage::Binary(data.to_vec());
		sender.send_message(&msg).unwrap();
	}
	

	let slck = Arc::new(Mutex::new(sender));
	{
		let mut s = sender_lck1.lock().unwrap();
		s.insert(port , slck.clone());
	}
	
	for message in receiver.incoming_messages() {
		let message = match message {
			Ok(p) => p,
			Err(_) => {
				log::warn!("client closed : [{}:{}]" ,ip , port );
				sender_lck1.lock().unwrap().remove(&port);
				return;
			},
		};
		
		match message {
			OwnedMessage::Close(_) => {
				sender_lck1.lock().unwrap().remove(&port);
				return;
			},
			OwnedMessage::Ping(ping) => {
				let message = OwnedMessage::Pong(ping);
				slck.lock().unwrap().send_message(&message).unwrap();
			},
			OwnedMessage::Text(text) => {
				ptyin.write_all(text.as_bytes()).unwrap();
				
			},
			OwnedMessage::Binary(data) => {

				if data.len() == 6{

					if data[0] == MAGIC_FLAG[0] && data[1] == MAGIC_FLAG[1] {

						let size = Box::new(libc::winsize{
							ws_row : makeword(data[2] , data[3]), 
							ws_col :  makeword(data[4] , data[5]) ,
							ws_xpixel : 0,
							ws_ypixel: 0, 
							
						});
						
						if set_termsize(slave , size) {
							std::process::exit(0);
						}

						continue;
					}
				}

				ptyin.write_all(data.as_slice()).unwrap();
			},
			_ => {},
		}
	}
}