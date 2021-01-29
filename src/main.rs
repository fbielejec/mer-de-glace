mod tree_hash;

use bytes::Bytes;
use chrono::{Utc, DateTime};
use std::time::Duration as Duration;
use flate2::Compression;
use flate2::write::GzEncoder;
use log::{info, warn};
use regex::Regex;
use rusoto_core::Region;
use rusoto_glacier::{Glacier, GlacierClient, DescribeVaultInput, CreateVaultInput, UploadArchiveInput, ArchiveCreationOutput};
use std::env;
use std::fs::{File, create_dir_all};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output};
use std::str::FromStr;
use tokio::time;

#[macro_use] extern crate lazy_static;

const ARCHIVE_ROOT: &str = "wordpress_backup";

lazy_static! {
    static ref RE: Regex = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
}

#[derive(Debug, Clone)]
struct Config {
    interval: u32,
    archive_rolling_period: u32,
    wordpress_directory: String,
    mysql_host: String,
    mysql_port: String,
    mysql_database: String,
    mysql_user: String,
    mysql_password: String,
    backups_directory: String,
    aws_region: String,
    aws_glacier_vault_name: String,
}

type AnyResult<T> = Result<T, anyhow::Error>;

#[tokio::main]
async fn main() -> AnyResult<()> {

    let config = Config {
        wordpress_directory: get_env_var ("WORDPRESS_DIRECTORY", None)?,
        mysql_host: get_env_var ("MYSQL_HOST", None)?,
        mysql_port: get_env_var ("MYSQL_PORT", Some (String::from ("3306")))?,
        mysql_database: get_env_var ("MYSQL_DATABASE", None)?,
        mysql_user: get_env_var ("MYSQL_USER", None)?,
        mysql_password: get_env_var ("MYSQL_PASSWORD", None)?,
        interval: get_env_var ("BACKUP_INTERVAL", Some (String::from ("7")))?.parse::<u32>()?,
        archive_rolling_period: get_env_var ("ARCHIVE_ROLLING_PERIOD", Some (String::from ("14")))?.parse::<u32>()?,
        backups_directory: get_env_var ("BACKUPS_DIRECTORY", Some (String::from ("backups")))?,
        aws_region: get_env_var ("AWS_REGION", Some (String::from ("us-east-2")))?,
        aws_glacier_vault_name: get_env_var ("AWS_GLACIER_VAULT", None)?
    };

    env::set_var("RUST_LOG", get_env_var ("VERBOSITY", Some (String::from ("info")))?);
    env_logger::init();

    info!("Running with {:#?}", &config);

    // ensure directory for backups
    create_dir_all (&config.backups_directory).unwrap_or_else(|_| panic!("Couldn't create directory: {}", &config.backups_directory));

    let mut interval = time::interval(Duration::from_secs(
        86400 * config.interval as u64
    ));
    loop {
        interval.tick().await;
        create_backup (&config).await?;
    }

}

async fn create_backup (config: &Config) -> AnyResult<()> {

    let today = Utc::now ();
    let date = today.format("%Y-%m-%d");

    let sql_dump_name = format!("dump_{}.sql", &date);
    let sql_dump_path = format!("{}/{}", &config.backups_directory, &sql_dump_name);

    // create sql dump
    let sql_dump = dump_sql (&config);
    write_to_file (&sql_dump, &sql_dump_path);

    // create gzip archive
    let archive_path = format!("{}/{}_{}.tar.gz", &config.backups_directory, ARCHIVE_ROOT, &date);
    let mut tar = create_archive (&archive_path)?;

    // add wordpress_directory to the archive
    tar.append_dir_all(format!("wordpress-html_{}", &date), &config.wordpress_directory)?;

    // add the sql dump to the archive
    let mut file = File::open(&sql_dump_path)?;
    tar.append_file(&sql_dump_name, &mut file)?;

    // close the archive
    tar.finish ()?;

    let glacier_client = GlacierClient::new(Region::from_str (&config.aws_region)?);

    ensure_vault (&glacier_client, &config.aws_glacier_vault_name).await?;

    let result = send_to_glacier (&archive_path,
                                  format!("Created: {}", &date),
                                  &glacier_client,
                                  &config.aws_glacier_vault_name).await?;

    info!("Archive succesfully stored in glacier with id: {}",
          &result.archive_id.unwrap_or_else(|| String::from ("unknown")));

    cleanup (&sql_dump_path, &config.backups_directory, &today, config.archive_rolling_period)?;

    info!("Done");

    Ok (())
}

fn cleanup (sql_dump_path: &str,
            backups_directory: &str,
            today: &DateTime<Utc>,
            rolling_period : u32)
            -> AnyResult<()> {

    fs::remove_file(sql_dump_path).unwrap_or_else (| why | { warn!("Could not remove {} {}", &sql_dump_path, why) });

    for entry in fs::read_dir(backups_directory)? {
        let path_buf = entry?.path ();
        let archive_name = path_buf.as_path ().display ().to_string ();
        let d = &RE.captures_iter(&archive_name).next ().unwrap () [0];
        let d = &format!("{} 00:00:00 +00:00", d);
        let archive_date = d.parse::<DateTime<Utc>>()?;

        let diff = (*today - archive_date).num_days ();
        if diff as u32 >= rolling_period {
            info! ("Archive {} is older than {} old, removing", archive_name, rolling_period);
            fs::remove_file(&archive_name).unwrap_or_else (| why | { warn!("Could not remove {} {}", &archive_name, why) });
        } else {
            info! ("Archive {} is {} days old", archive_name, diff);
        }

    }

    Ok (())
}

async fn send_to_glacier (file_path : &str,
                          description : String,
                          client : &GlacierClient,
                          vault_name : &str)
                          -> AnyResult<ArchiveCreationOutput> {

    let hash : String = match tree_hash::tree_hash(file_path) {
        Ok(hash_bytes) => {
            tree_hash::to_hex_string(&hash_bytes)
        },
        Err(_) => panic!("Error calculating tree hash")
    };

    info!("Archive content hash: {}", &hash);

    let mut file : File = File::open(&file_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let bytes : Bytes = Bytes::from (buffer);

    let request = UploadArchiveInput {
        account_id: "-".to_string(),
        archive_description: Some (description),
        body: Some (bytes),
        checksum: Some (hash),
        vault_name: String::from (vault_name)
    };

    let result = match client.upload_archive (request).await {
        Ok (res) => res,
        Err (err) => panic!("Error when uploading {} to glacier: {}", file_path, err)
    };

    Ok (result)
}

async fn ensure_vault (client : &GlacierClient, vault_name : &str) -> AnyResult<()> {

    let request = DescribeVaultInput {
        account_id: "-".to_string(),
        vault_name: String::from (vault_name),
    };

    match client.describe_vault (request).await {
        Ok (result) => {
            info! ("Glacier vault exists: {:#?}", result);
        },
        Err (err) => {
            warn! ("Glacier vault {} not found: {:#?}", vault_name, err);
            let request = CreateVaultInput {
                account_id: "-".to_string(),
                vault_name: String::from (vault_name),
            };
            match client.create_vault (request).await {
                Ok (result) => {
                    info! ("Created glacier vault: {:#?}", result);
                },
                Err (err) => {
                    panic! ("Could not create glacier vault {}", err);
                }
            };
        }
    };

    Ok (())
}

fn create_archive (path : &str)
                   -> AnyResult<tar::Builder<flate2::write::GzEncoder<std::fs::File>>> {
    let tar_gz = File::create(path)?;
    let encoder = GzEncoder::new(tar_gz, Compression::default());
    Ok (tar::Builder::new(encoder))
}

// TODO : spawn as thread
fn dump_sql (config: &Config) -> Vec<u8> {

    let Config { mysql_host, mysql_port, mysql_user, mysql_password, mysql_database, .. } = config;

    let output : Output = Command::new("mysqldump")
        .arg("-h")
        .arg(&mysql_host)
        .arg("--port")
        .arg(&mysql_port)
        .arg("-u")
        .arg(&mysql_user)
        .arg(format!("-p{}", &mysql_password))
        .arg("--databases")
        .arg(&mysql_database)
        .output()
        .expect("Failed to execute mysqldump");

    info!("Succesfully dumped SQL data");

    output.stdout
}

fn write_to_file (content: &[u8], path : &str) {
    match File::create(Path::new(&path)) {
        Err(why) => panic!("Couldn't create {:#?}: {}", path, why),
        Ok(mut file) => {
            match file.write_all(content) {
                Err(why) => panic!("Couldn't write to {}: {}", path, why),
                Ok(_) => info!("Successfully wrote to file {}", path),
            };
        }
    }
}

fn get_env_var (var : &str, default: Option<String> ) -> AnyResult<String> {
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
