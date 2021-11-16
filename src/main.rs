use log::LevelFilter;
use simple_logger::SimpleLogger;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod xnix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use xnix::{bind , connect ,rbind ,rconnect };
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
use win::{bind , connect ,rbind ,rconnect };

fn usage () {
	println!("Cliws - Lightweight bind/reverse PTY shell implementation by Rust");
	println!("https://github.com/b23r0/Cliws");
	println!("Usage: cliws [-p listen port] [-c ws address] [-l reverse port] [-r reverse addr] [command]");
}

fn main() {

	SimpleLogger::new().with_colors(true).init().unwrap();
	::log::set_max_level(LevelFilter::Info);

	let arg_count = std::env::args().count();

	if  arg_count == 1{
		usage();
		return;
	}

	let first  = std::env::args().nth(1).expect("parameter not enough");

	match first.as_str() {
		"-l" => {

			let port = match std::env::args().nth(2) {
				None => {
					log::error!("not found listen port . eg : cliws -l 8000");
					return;
				},
				Some(p) => p
			};

			rbind(port);
			return;
		},
		"-r" => {
			let address = match std::env::args().nth(2) {
				None => {
					log::error!("not found reverse connection address . eg : cliws -r ws://127.0.0.1:8000 bash -i");
					return;
				},
				Some(p) => p
			};

			let subprocess = match std::env::args().nth(3) {
				None => {
					log::error!("not found command . eg : cliws -r ws://127.0.0.1:8000 bash -i");
					return;
				},
				Some(p) => p
			};

			let mut fullargs : Vec<String> = Vec::new();
			for i in 4..arg_count {
		
				let s = std::env::args().nth(i).expect("parse parameter faild");
				fullargs.push(s);
			}
			rconnect(address, subprocess, fullargs);
			return;
		},
		"-c" => {
			let connect_addr = match std::env::args().nth(2) {
				None => {
					log::error!("not found connection address . eg : cliws -c ws://127.0.0.1:8000");
					return;
				},
				Some(p) => p
			};
			connect(connect_addr);
			return;
		},
		"-p" => {
			let port = match std::env::args().nth(2) {
				None => {
					log::error!("not found listen port . eg : cliws -p 8000 bash -i");
					return;
				},
				Some(p) => p
			};
			let mut fullargs : Vec<String> = Vec::new();

			let subprocess = match std::env::args().nth(3) {
				None => {
					log::error!("not found command . eg : cliws -p 8000 bash -i");
					return;
				},
				Some(p) => p
			};
			
			for i in 4..arg_count {
		
				let s = std::env::args().nth(i).expect("parse parameter faild");
				fullargs.push(s);
			}
			bind(port, subprocess, fullargs);
			return;
		},

		_ => {
			usage();
			return;
		}
	}
}
