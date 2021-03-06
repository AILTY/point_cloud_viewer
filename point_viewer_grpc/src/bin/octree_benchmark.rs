// Copyright 2018 Google Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use futures::future::Future;
use futures::Stream;
use grpcio::{ChannelBuilder, Environment};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use point_viewer::data_provider::{DataProviderFactory, OnDiskDataProvider};
use point_viewer::iterator::{ParallelIterator, PointQuery};
use point_viewer::octree::Octree;
use point_viewer_grpc::proto_grpc::OctreeClient;
use point_viewer_grpc::service::start_grpc_server;
use point_viewer_grpc_proto_rust::proto;

// size for batch
const BATCH_SIZE: usize = 1_000_000;
fn main() {
    let matches = clap::App::new("octree_benchmark")
        .args(&[
            clap::Arg::with_name("port")
                .about("Port for the server to listen on for connections. [50051]")
                .long("port")
                .takes_value(true),
            clap::Arg::with_name("no-client")
                .about("Do not actually send points, only read them on the server.")
                .long("no-client")
                .takes_value(false),
            clap::Arg::with_name("num-points")
                .about("Number of points to stream. [50000000]")
                .long("num-points")
                .takes_value(true),
            clap::Arg::with_name("num-threads")
                .about("Number of threads, num(cpus) - 1 by default")
                .long("num-threads")
                .takes_value(true),
            clap::Arg::with_name("buffer-size")
                .about("Buffer capacity, 4 by default")
                .long("buffer")
                .takes_value(true),
            clap::Arg::with_name("octree_directory")
                .about("Input directory of the octree directory to serve.")
                .index(1)
                .required(true),
        ])
        .get_matches();

    let octree_directory = PathBuf::from(
        matches
            .value_of("octree_directory")
            .expect("octree_directory not given"),
    );
    let num_points = usize::from_str(matches.value_of("num-points").unwrap_or("50000000"))
        .expect("num-points needs to be a number");
    let num_threads = usize::from_str(
        matches
            .value_of("num-threads")
            .unwrap_or(&(std::cmp::max(1, num_cpus::get() - 1)).to_string()),
    )
    .expect("num-threads needs to be a number");
    let buffer_size = usize::from_str(matches.value_of("buffer-size").unwrap_or("4"))
        .expect("buffer-size needs to be a number");
    if matches.is_present("no-client") {
        server_benchmark(&octree_directory, num_points, num_threads, buffer_size)
    } else {
        let port = matches.value_of_t("port").unwrap_or(50051);
        full_benchmark(&octree_directory, num_points, port)
    }
}

fn server_benchmark(
    octree_directory: &Path,
    num_points: usize,
    num_threads: usize,
    buffer_size: usize,
) {
    let octree = Octree::from_data_provider(Box::new(OnDiskDataProvider {
        directory: octree_directory.into(),
    }))
    .unwrap_or_else(|_| {
        panic!(
            "Could not create octree from '{}'",
            octree_directory.display()
        )
    });
    let mut counter: usize = 0;
    let mut points_streamed_m = 0;
    let all_points = PointQuery {
        attributes: vec!["color", "intensity"],
        ..Default::default()
    };
    let octree_slice: &[Octree] = std::slice::from_ref(&octree);
    let mut parallel_iterator = ParallelIterator::new(
        octree_slice,
        &all_points,
        BATCH_SIZE,
        num_threads,
        buffer_size,
    );
    eprintln!("Server benchmark:");
    let _result = parallel_iterator.try_for_each_batch(move |points_batch| {
        counter += points_batch.position.len();

        if points_streamed_m < counter / 1_000_000 {
            points_streamed_m = counter / 1_000_000;
            eprintln!("Streamed {}M points", points_streamed_m)
        };
        if counter >= num_points {
            std::process::exit(0)
        }
        Ok(())
    });
}

// this test works with number of threads = num cpus -1 and batch size such that the proto is less than 4 MB
fn full_benchmark(octree_directory: &Path, num_points: usize, port: u16) {
    let data_provider_factory = DataProviderFactory::new();
    let mut server = start_grpc_server("0.0.0.0", port, octree_directory, data_provider_factory);
    server.start();

    let env = Arc::new(Environment::new(1));
    let ch = ChannelBuilder::new(env).connect(&format!("localhost:{}", port));
    let client = OctreeClient::new(ch);

    let req = proto::GetAllPointsRequest::new();
    let receiver = client.get_all_points(&req).unwrap();

    let mut counter: usize = 0;

    'outer: for rep in receiver.wait() {
        for _pos in rep.expect("Stream error").get_positions().iter() {
            if counter % 1_000_000 == 0 {
                eprintln!("Streamed {}M points", counter / 1_000_000);
            }
            counter += 1;
            if counter == num_points {
                break 'outer;
            }
        }
    }

    let _ = server.shutdown().wait();
}
