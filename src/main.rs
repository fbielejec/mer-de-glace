use chrono::{Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use log::{debug, info, warn, error};
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs::{File, create_dir_all};
use std::io::prelude::*;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration ;

#[derive(Debug, Clone)]
struct Config {
    wordpress_directory: String,
    mysql_host: String,
    mysql_port: String,
    mysql_database: String,
    mysql_user: String,
    mysql_password: String,
    backups_directory: String
}

fn main() -> Result<(), anyhow::Error> {

    let config = Config {
        wordpress_directory: get_env_var ("WORDPRESS_DIRECTORY", None)?,
        mysql_host: get_env_var ("MYSQL_HOST", None)?,
        mysql_port: get_env_var ("MYSQL_PORT", None)?,
        mysql_database: get_env_var ("MYSQL_DATABASE", None)?,
        mysql_user: get_env_var ("MYSQL_USER", None)?,
        mysql_password: get_env_var ("MYSQL_PASSWORD", None)?,
        backups_directory: get_env_var ("BACKUPS_DIRECTORY", Some (String::from ("backups")))?,
    };

    env::set_var("RUST_LOG", get_env_var ("VERBOSITY", Some (String::from ("info")))?);
    env_logger::init();

    info!("Running with {:#?}", &config);

    let output : Output = Command::new("mysqldump")
        .arg("-h")
        .arg(&config.mysql_host)
        .arg("--port")
        .arg(&config.mysql_port)
        .arg("-u")
        .arg(&config.mysql_user)
        .arg(format!("-p{}", &config.mysql_password))
        .arg("--databases")
        .arg(&config.mysql_database)
        .output()
        .expect("Failed to execute mysqldump");
    let sql_dump = output.stdout;

    create_dir_all (&config.backups_directory).expect (&format! ("Couldn't create directory: {}", &config.backups_directory));

    let today = Utc::now ();
    let date = today.format("%Y-%m-%d");

    let sql_dump_name = format!("dump_{}.sql", &date);
    let sql_dump_path = format!("{}/{}.sql", &config.backups_directory, &sql_dump_name);
    // let path = Path::new(&p);
    // let display = path.display();

    // Open a file in write-only mode
    let mut file = match File::create(Path::new(&sql_dump_path)) {
        Err(why) => panic!("Couldn't create {:#?}: {}", sql_dump_path, why),
        Ok(file) => {
            info!("Created backup file {}", &sql_dump_path);
            file
        },
    };

    match file.write_all(&sql_dump) {
        Err(why) => panic!("Couldn't write to {}: {}", sql_dump_path, why),
        Ok(_) => info!("Successfully wrote to {}", sql_dump_path),
    };

    // gzip the directory
    let backup_archive_path = format!("{}/wordpress_backup_{}.tar.gz", &config.backups_directory, &date);
    let tar_gz = File::create(backup_archive_path)?;
    let encoder = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(encoder);

    tar.append_dir_all("wordpress-html", &config.wordpress_directory)?;

    // add the dump to directory
    let mut file = File::open(&sql_dump_path)?;
    tar.append_file(&sql_dump_name, &mut file)?;

    tar.finish ()?;

    // tar.append_dir_all("mysql", &config.wordpress_directory)?;

    // info!("{:?}", &sql_dump);
    // print_type_of (&sql_dump.get (0).unwrap ());

    info!("Done");
    Ok (())
}

fn get_env_var (var : &str, default: Option<String> ) -> Result<String, anyhow::Error> {
    match env::var(var) {
        Ok (v) => Ok (v),
        Err (_) => {
            match default {
                None => panic! ("Missing ENV variable: {} not defined in environment", var),
                Some (d) => Ok (d)
            }
        }
    }
}

pub fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>())
}
