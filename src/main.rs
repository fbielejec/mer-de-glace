mod tree_hash;

use bytes::Bytes;
use chrono::{Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use log::{info, warn};
use rusoto_core::Region;
use rusoto_glacier::{Glacier, GlacierClient, DescribeVaultInput, CreateVaultInput, UploadArchiveInput, ArchiveCreationOutput};
use std::env;
use std::fs::{File, create_dir_all};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output};
use std::str::FromStr;

#[derive(Debug, Clone)]
struct Config {
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


// TODO : refactor to use (async) functions
#[tokio::main]
async fn main() -> AnyResult<()> {

    let today = Utc::now ();
    let date = today.format("%Y-%m-%d");

    let config = Config {
        wordpress_directory: get_env_var ("WORDPRESS_DIRECTORY", None)?,
        mysql_host: get_env_var ("MYSQL_HOST", None)?,
        mysql_port: get_env_var ("MYSQL_PORT", None)?,
        mysql_database: get_env_var ("MYSQL_DATABASE", None)?,
        mysql_user: get_env_var ("MYSQL_USER", None)?,
        mysql_password: get_env_var ("MYSQL_PASSWORD", None)?,
        backups_directory: get_env_var ("BACKUPS_DIRECTORY", Some (String::from ("backups")))?,
        aws_region: get_env_var ("AWS_REGION", Some (String::from ("us-east-2")))?,
        aws_glacier_vault_name: get_env_var ("AWS_GLACIER_VAULT", None)?
    };

    env::set_var("RUST_LOG", get_env_var ("VERBOSITY", Some (String::from ("info")))?);
    env_logger::init();

    info!("Running with {:#?}", &config);

    create_dir_all (&config.backups_directory).expect (&format! ("Couldn't create directory: {}", &config.backups_directory));

    let sql_dump_name = format!("dump_{}.sql", &date);
    let sql_dump_path = format!("{}/{}", &config.backups_directory, &sql_dump_name);

    // create sql dump
    let sql_dump = dump_sql (&config);
    write_to_file (&sql_dump, &sql_dump_path)?;

    // create gzip archive
    let archive_path = format!("{}/wordpress_backup_{}.tar.gz", &config.backups_directory, &date);
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

    // send archive to glacier
    // let mut archive : File = File::open(&archive_path)?;

    let result = send_to_glacier (//archive,
                                  &archive_path,
                                  format!("Created: {}", &date),
                                  &glacier_client,
                                  &config.aws_glacier_vault_name).await?;

    info!("Archive succesfully stored in glacier: {} with id: {}", &result.location.unwrap_or (String::from ("unknown")), &result.archive_id.unwrap_or (String::from ("unknown")));

    // TODO : cleanup
    // sql dump file
    // archives older than x amount of time

    info!("Done");
    Ok (())
}

// TODO : return id
async fn send_to_glacier (file_path : &str, description : String, client : &GlacierClient, vault_name : &str) -> AnyResult<ArchiveCreationOutput> {

    let hash : String = match tree_hash::tree_hash(file_path) {
        Ok(hash_bytes) => {
            tree_hash::to_hex_string(&hash_bytes)
        },
        Err(_) => panic!("Error calculating tree hash")
    };

    info!("content hash: {}", &hash);

    let mut file : File = File::open(&file_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let bytes : Bytes = Bytes::from (buffer);

    // info!("here @1 {:#?}", bytes);

    let request = UploadArchiveInput {
        account_id: "-".to_string(),
        archive_description: Some (description),
        body: Some (bytes),
        checksum: Some (hash),
        vault_name: String::from (vault_name),
        ..Default::default()
    };

    // info!("here @2");

    let result = client.upload_archive (request).await?;

    // info!("here @3");

    Ok (result)
}

async fn ensure_vault (client : &GlacierClient, vault_name : &str) -> AnyResult<()> {

    let request = DescribeVaultInput {
        account_id: "-".to_string(),
        vault_name: String::from (vault_name),
    };

    match client.describe_vault (request).await {
        Ok (result) => {
            info! ("Vault exists: {:#?}", result);
        },
        Err (err) => {
            warn! ("Vault {} not found: {:#?}", vault_name, err);
            let request = CreateVaultInput {
                account_id: "-".to_string(),
                vault_name: String::from (vault_name),
            };
            match client.create_vault (request).await {
                Ok (result) => {
                    info! ("Created vault: {:#?}", result);
                },
                Err (err) => {
                    panic! ("Could not create vault {}", err);
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

fn write_to_file (content: &Vec<u8>, path : &str) -> AnyResult<()> {
    match File::create(Path::new(&path)) {
        Err(why) => panic!("Couldn't create {:#?}: {}", path, why),
        Ok(mut file) => {
            match file.write_all(content) {
                Err(why) => panic!("Couldn't write to {}: {}", path, why),
                Ok(_) => info!("Successfully wrote to file {}", path),
            };
        }
    };

    Ok (())
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
