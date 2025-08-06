use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(short, long)]
    mnemonic: String,
    #[arg(short, long)]
    address: String,
}

fn main() {
    let args = Args::parse();
}
