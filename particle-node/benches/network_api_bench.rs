/*
 * Copyright 2020 Fluence Labs Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::convert::Infallible;
use std::mem;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Waker;
use std::time::{Duration, Instant};

use async_std::task;
use async_std::task::{spawn, JoinHandle};
use criterion::async_executor::AsyncStdExecutor;
use criterion::{criterion_group, criterion_main, BatchSize};
use criterion::{BenchmarkId, Criterion, Throughput};
use eyre::WrapErr;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::{FutureExt, SinkExt};
use humantime_serde::re::humantime::format_duration as pretty;
use libp2p::{PeerId, Swarm};

use aquamarine::{
    AquaRuntime, AquamarineApi, AquamarineBackend, AquamarineVM, InterpreterOutcome, SendParticle,
    StepperEffects, VmConfig, VmPoolConfig,
};
use connection_pool::ConnectionPoolApi;
use fluence_libp2p::types::{BackPressuredInlet, OneshotOutlet};
use fluence_libp2p::{build_memory_transport, RandomPeerId};
use kademlia::{Kademlia, KademliaApi, KademliaApiInlet, KademliaConfig};
use libp2p::core::identity::ed25519::Keypair;
use libp2p::core::identity::Keypair::Ed25519;
use particle_closures::{HostClosures, NodeInfo};
use particle_node::{ConnectionPoolCommand, Connectivity, KademliaCommand, NetworkApi};
use particle_protocol::{Contact, Particle};
use script_storage::ScriptStorageApi;
use server_config::ServicesConfig;
use test_utils::{
    create_memory_maddr, create_swarm, make_swarms, make_tmp_dir, now_ms, put_aquamarine,
    SwarmConfig,
};
use tracing_futures::Instrument;
use tracing_log::LogTracer;
use trust_graph::{InMemoryStorage, TrustGraph};

const TIMEOUT: Duration = Duration::from_secs(10);
const PARALLELISM: Option<usize> = Some(16);

async fn particles(n: usize) -> BackPressuredInlet<Particle> {
    let (mut outlet, inlet) = mpsc::channel(n * 2);

    let last_particle = std::iter::once({
        let mut p = Particle::default();
        p.id = String::from("last");
        Ok(p)
    });
    fn particle(n: usize) -> Particle {
        Particle {
            timestamp: now_ms() as u64,
            ttl: 10000,
            id: n.to_string(),
            script: String::from(r#"(call %init_peer_id% ("op" "identity") ["hello"] result)"#),
            ..<_>::default()
        }
    }
    let mut particles = futures::stream::iter((0..n).map(|i| Ok(particle(i))).chain(last_particle));
    outlet.send_all(&mut particles).await.unwrap();
    mem::forget(outlet);

    inlet
}

fn kademlia_api() -> (KademliaApi, JoinHandle<()>) {
    use futures::StreamExt;

    let (outlet, mut inlet) = mpsc::unbounded();
    let api = KademliaApi { outlet };

    let handle = spawn(futures::future::poll_fn::<(), _>(move |cx| {
        use std::task::Poll;

        let mut wake = false;
        while let Poll::Ready(Some(cmd)) = inlet.poll_next_unpin(cx) {
            wake = true;
            // TODO: this shouldn't be called
            match cmd {
                KademliaCommand::AddContact { .. } => {}
                KademliaCommand::LocalLookup { out, .. } => out.send(vec![]).unwrap(),
                KademliaCommand::Bootstrap { out, .. } => out.send(Ok(())).unwrap(),
                KademliaCommand::DiscoverPeer { out, .. } => out.send(Ok(vec![])).unwrap(),
                KademliaCommand::Neighborhood { out, .. } => out.send(Ok(vec![])).unwrap(),
            }
        }

        if wake {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }));

    (api, handle)
}

// fn real_kademlia_api(keypair: Keypair, peer_id: PeerId) -> (KademliaApi, KademliaApiInlet) {
//     let kad_config = KademliaConfig {
//         peer_id,
//         keypair: keypair.clone(),
//         kad_config: server_config::KademliaConfig {
//             max_packet_size: Some(100 * 4096 * 4096), // 100Mb
//             query_timeout: Duration::from_secs(3),
//             replication_factor: None,
//             connection_idle_timeout: Some(Duration::from_secs(2_628_000_000)), // ~month
//             peer_fail_threshold: 3,
//             ban_cooldown: Duration::from_secs(60),
//         },
//     };
//
//     let trust_graph = {
//         let storage = InMemoryStorage::new_in_memory(vec![]);
//         TrustGraph::new(storage)
//     };
//     let kademlia = Kademlia::new(kad_config, trust_graph, None);
//     let (kademlia_api, kademlia): (KademliaApi, KademliaApiInlet) = kademlia.into();
//
//     let transport = build_memory_transport(Ed25519(keypair));
//
//     let mut swarm: Swarm<KademliaApiInlet> = Swarm::new(transport, behaviour, local_peer_id);
//     let addr = create_memory_maddr();
//     Swarm::listen_on(&mut swarm, addr).expect("listen_on");
//
//     swarm.add_addresses()
//
//     // task::spawn()
//
//     (kademlia_api, kademlia)
// }

struct Stops(Vec<OneshotOutlet<()>>);
impl Stops {
    pub async fn cancel(self) {
        for stop in self.0 {
            stop.send(()).expect("send stop")
        }
    }
}

fn real_kademlia_api(network_size: usize) -> (KademliaApi, Stops) {
    let mut swarms = make_swarms(network_size).into_iter();

    let swarm = swarms.next().unwrap();
    let kad_api = swarm.connectivity.kademlia;
    let stop = swarm.outlet;

    let stops = std::iter::once(stop)
        .chain(swarms.map(|s| s.outlet))
        .collect::<Vec<_>>();

    (kad_api, Stops(stops))
}

fn connection_pool_api(num_particles: usize) -> (ConnectionPoolApi, JoinHandle<()>) {
    use futures::StreamExt;

    let (outlet, mut inlet) = mpsc::unbounded();
    let api = ConnectionPoolApi {
        outlet,
        send_timeout: TIMEOUT,
    };

    let counter = AtomicUsize::new(0);

    let future = spawn(futures::future::poll_fn(move |cx| {
        use std::task::Poll;

        let mut wake = false;
        while let Poll::Ready(Some(cmd)) = inlet.poll_next_unpin(cx) {
            wake = true;

            match cmd {
                ConnectionPoolCommand::Connect { out, .. } => out.send(true).unwrap(),
                ConnectionPoolCommand::Send { out, .. } => {
                    let num = counter.fetch_add(1, Ordering::Relaxed);
                    out.send(true).unwrap();
                    if num == num_particles - 1 {
                        return Poll::Ready(());
                    }
                }
                ConnectionPoolCommand::Dial { out, .. } => out.send(None).unwrap(),
                ConnectionPoolCommand::Disconnect { out, .. } => out.send(true).unwrap(),
                ConnectionPoolCommand::IsConnected { out, .. } => out.send(true).unwrap(),
                ConnectionPoolCommand::GetContact { peer_id, out } => {
                    out.send(Some(Contact::new(peer_id, vec![]))).unwrap()
                }
                ConnectionPoolCommand::CountConnections { out, .. } => out.send(0).unwrap(),
                ConnectionPoolCommand::LifecycleEvents { .. } => {}
            }
        }

        if wake {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }));

    (api, future)
}

fn aquamarine_api() -> (AquamarineApi, JoinHandle<()>) {
    use futures::StreamExt;

    let (outlet, mut inlet) = mpsc::channel(100);

    let api = AquamarineApi::new(outlet, TIMEOUT);

    let handle = spawn(futures::future::poll_fn::<(), _>(move |cx| {
        use std::task::Poll;

        let mut wake = false;
        while let Poll::Ready(Some(a)) = inlet.poll_next_unpin(cx) {
            wake = true;
            let (particle, ch) = a;
            ch.send(Ok(StepperEffects {
                particles: vec![SendParticle {
                    target: particle.init_peer_id,
                    particle,
                }],
            }))
            .unwrap();
        }

        if wake {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }));

    (api, handle)
}

fn aquamarine_with_backend(
    pool_size: usize,
    delay: Option<Duration>,
) -> (AquamarineApi, JoinHandle<()>) {
    struct EasyVM {
        delay: Option<Duration>,
    }

    impl AquaRuntime for EasyVM {
        type Config = Option<Duration>;
        type Error = Infallible;

        fn create_runtime(
            delay: Option<Duration>,
            _: Waker,
        ) -> BoxFuture<'static, Result<Self, Self::Error>> {
            futures::future::ok(EasyVM { delay }).boxed()
        }

        fn into_effects(_: Result<InterpreterOutcome, Self::Error>, p: Particle) -> StepperEffects {
            StepperEffects {
                particles: vec![SendParticle {
                    target: p.init_peer_id,
                    particle: p,
                }],
            }
        }

        fn call(
            &mut self,
            init_user_id: PeerId,
            _aqua: String,
            data: Vec<u8>,
            _particle_id: String,
        ) -> Result<InterpreterOutcome, Self::Error> {
            if let Some(delay) = self.delay {
                std::thread::sleep(delay);
            }

            Ok(InterpreterOutcome {
                ret_code: 0,
                error_message: "".to_string(),
                data: data.into(),
                next_peer_pks: vec![init_user_id.to_string()],
            })
        }
    }

    let config = VmPoolConfig {
        pool_size,
        execution_timeout: TIMEOUT,
    };
    let (backend, api): (AquamarineBackend<EasyVM>, _) = AquamarineBackend::new(config, delay);
    let handle = backend.start();

    (api, handle)
}

fn aquamarine_with_vm<C>(
    pool_size: usize,
    connectivity: C,
    local_peer_id: PeerId,
    interpreter: PathBuf,
) -> (AquamarineApi, JoinHandle<()>)
where
    C: Clone + Send + Sync + 'static + AsRef<KademliaApi> + AsRef<ConnectionPoolApi>,
{
    let tmp_dir = make_tmp_dir();

    let node_info = NodeInfo {
        external_addresses: vec![],
    };
    let script_storage_api = ScriptStorageApi {
        outlet: mpsc::unbounded().0,
    };
    let services_config = ServicesConfig::new(
        local_peer_id,
        tmp_dir.join("services"),
        <_>::default(),
        RandomPeerId::random(),
    )
    .wrap_err("create service config")
    .unwrap();
    let host_closures =
        HostClosures::new(connectivity, script_storage_api, node_info, services_config);

    let pool_config = VmPoolConfig {
        pool_size,
        execution_timeout: TIMEOUT,
    };
    let vm_config = VmConfig {
        current_peer_id: local_peer_id,
        workdir: tmp_dir.join("workdir"),
        air_interpreter: interpreter,
        services_dir: tmp_dir.join("services_dir"),
        particles_dir: tmp_dir.join("particles_dir"),
    };
    let (stepper_pool, stepper_pool_api): (AquamarineBackend<AquamarineVM>, _) =
        AquamarineBackend::new(pool_config, (vm_config, host_closures.descriptor()));

    let handle = stepper_pool.start();

    (stepper_pool_api, handle)
}

#[allow(dead_code)]
async fn network_api(particles_num: usize) -> (NetworkApi, Vec<JoinHandle<()>>) {
    let particle_stream: BackPressuredInlet<Particle> = particles(particles_num).await;
    let particle_parallelism: usize = 1;
    let (kademlia, kad_handle) = kademlia_api();
    let (connection_pool, cp_handle) = connection_pool_api(1000);
    let bootstrap_frequency: usize = 1000;
    let particle_timeout: Duration = Duration::from_secs(5);

    let api: NetworkApi = NetworkApi::new(
        particle_stream,
        particle_parallelism,
        kademlia,
        connection_pool,
        bootstrap_frequency,
        particle_timeout,
    );
    (api, vec![cp_handle, kad_handle])
}

fn connectivity(num_particles: usize) -> (Connectivity, BoxFuture<'static, ()>, JoinHandle<()>) {
    let (kademlia, kad_handle) = kademlia_api();
    let (connection_pool, cp_handle) = connection_pool_api(num_particles);
    let connectivity = Connectivity {
        kademlia,
        connection_pool,
    };

    (connectivity, cp_handle.boxed(), kad_handle)
}

fn connectivity_with_real_kad(
    num_particles: usize,
    network_size: usize,
) -> (Connectivity, BoxFuture<'static, ()>, Stops) {
    let (kademlia, stops) = real_kademlia_api(network_size);
    let (connection_pool, cp_handle) = connection_pool_api(num_particles);
    let connectivity = Connectivity {
        kademlia,
        connection_pool,
    };

    (connectivity, cp_handle.boxed(), stops)
}

async fn process_particles(
    num_particles: usize,
    parallelism: Option<usize>,
    particle_timeout: Duration,
) {
    let (con, finish, kademlia) = connectivity(num_particles);
    let (aquamarine, aqua_handle) = aquamarine_api();
    let (sink, _) = mpsc::unbounded();

    let particle_stream: BackPressuredInlet<Particle> = particles(num_particles).await;
    let process = spawn(con.clone().process_particles(
        parallelism,
        particle_stream,
        aquamarine,
        sink,
        particle_timeout,
    ));
    finish.await;

    process.cancel().await;
    kademlia.cancel().await;
    aqua_handle.cancel().await;
}

fn thousand_particles_bench(c: &mut Criterion) {
    c.bench_function("thousand_particles", move |b| {
        let n = 1000;
        let particle_timeout = TIMEOUT;
        let parallelism = PARALLELISM;

        b.to_async(AsyncStdExecutor)
            .iter(move || process_particles(n, parallelism, particle_timeout))
    });
}

fn particle_throughput_bench(c: &mut Criterion) {
    let parallelism = PARALLELISM;
    let mut group = c.benchmark_group("particle_throughput");
    for size in [1, 1000, 2 * 1000, 4 * 1000, 8 * 1000, 16 * 1000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &n| {
            b.to_async(AsyncStdExecutor)
                .iter(move || process_particles(n, parallelism, TIMEOUT))
        });
    }
}

async fn process_particles_with_delay(
    num_particles: usize,
    pool_size: usize,
    call_delay: Option<Duration>,
    particle_parallelism: Option<usize>,
    particle_timeout: Duration,
) {
    let (con, future, kademlia) = connectivity(num_particles);
    let (aquamarine, aqua_handle) = aquamarine_with_backend(pool_size, call_delay);
    let (sink, _) = mpsc::unbounded();
    let particle_stream: BackPressuredInlet<Particle> = particles(num_particles).await;
    let process = spawn(con.clone().process_particles(
        particle_parallelism,
        particle_stream,
        aquamarine,
        sink,
        particle_timeout,
    ));
    future.await;

    process.cancel().await;
    kademlia.cancel().await;
    aqua_handle.cancel().await;
}

async fn process_particles_with_vm(
    num_particles: usize,
    pool_size: usize,
    particle_parallelism: Option<usize>,
    particle_timeout: Duration,
    interpreter: PathBuf,
) {
    let peer_id = RandomPeerId::random();

    let (con, future, kademlia) = connectivity(num_particles);
    let (aquamarine, aqua_handle) =
        aquamarine_with_vm(pool_size, con.clone(), peer_id, interpreter);
    let (sink, _) = mpsc::unbounded();
    let particle_stream: BackPressuredInlet<Particle> = particles(num_particles).await;
    let process = spawn(con.clone().process_particles(
        particle_parallelism,
        particle_stream,
        aquamarine,
        sink,
        particle_timeout,
    ));
    future.await;

    process.cancel().await;
    kademlia.cancel().await;
    aqua_handle.cancel().await;
}

fn thousand_particles_with_aquamarine_bench(c: &mut Criterion) {
    c.bench_function("thousand_particles_with_aquamarine", move |b| {
        let n = 1000;
        let pool_size = 1;
        let call_time = Some(Duration::from_nanos(1));
        let particle_parallelism = PARALLELISM;
        let particle_timeout = TIMEOUT;

        b.to_async(AsyncStdExecutor).iter(move || {
            process_particles_with_delay(
                n,
                pool_size,
                call_time,
                particle_parallelism,
                particle_timeout,
            )
        })
    });
}

fn particle_throughput_with_delay_bench(c: &mut Criterion) {
    let particle_parallelism = PARALLELISM;
    let particle_timeout = TIMEOUT;

    let mut group = c.benchmark_group("particle_throughput_with_delay");
    for &num in [1, 1000, 4 * 1000, 8 * 1000].iter() {
        for delay in [None, Some(Duration::from_millis(1))].iter() {
            for &pool_size in [1, 2, 4, 16].iter() {
                group.throughput(Throughput::Elements(num as u64));
                let bid = {
                    let delay = delay.unwrap_or(Duration::from_nanos(0));
                    BenchmarkId::from_parameter(format!("{}:{}@{}", num, pretty(delay), pool_size))
                };
                group.bench_with_input(bid, &(delay, num), |b, (&delay, n)| {
                    b.to_async(AsyncStdExecutor).iter(move || {
                        process_particles_with_delay(
                            *n,
                            pool_size,
                            delay,
                            particle_parallelism,
                            particle_timeout,
                        )
                    })
                });
            }
        }
    }
}

fn particle_throughput_with_kad_bench(c: &mut Criterion) {
    use tracing::Dispatch;
    use tracing_timing::{Builder, Histogram};

    // let subscriber = FmtSubscriber::builder()
    //     // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
    //     // will be written to stdout.
    //     .with_max_level(Level::TRACE)
    //     // completes the builder.
    //     .finish();

    // LogTracer::init_with_filter(log::LevelFilter::Error).expect("Failed to set logger");

    let subscriber = Builder::default()
        .no_span_recursion()
        .build(|| Histogram::new_with_max(1_000_000, 2).unwrap());
    let downcaster = subscriber.downcaster();
    let dispatcher = Dispatch::new(subscriber);
    let d2 = dispatcher.clone();
    tracing::dispatcher::set_global_default(dispatcher.clone())
        .expect("setting default dispatch failed");
    // tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let particle_parallelism = PARALLELISM;
    let particle_timeout = TIMEOUT;

    let tmp_dir = make_tmp_dir();
    let interpreter = put_aquamarine(tmp_dir.join("modules"));

    let mut group = c.benchmark_group("particle_throughput_with_kad");
    let num = 100;
    let pool_size = 4;
    let network_size = 10;

    tracing::info_span!("whole_bench").in_scope(|| {
        // for &num in [1, 1000, 4 * 1000, 8 * 1000].iter() {
        //     for &pool_size in [1, 2, 4, 16].iter() {
        group.throughput(Throughput::Elements(num as u64));
        group.sample_size(10);
        let bid = { BenchmarkId::from_parameter(format!("{}@{}", num, pool_size)) };
        group.bench_with_input(bid, &num, |b, &n| {
            let interpreter = interpreter.clone();
            b.iter_batched(
                || {
                    let interpreter = interpreter.clone();
                    let peer_id = RandomPeerId::random();

                    let (con, finish_fut, kademlia) = connectivity_with_real_kad(n, network_size);
                    // let (aquamarine, aqua_handle) =
                    //     aquamarine_with_vm(pool_size, con.clone(), peer_id, interpreter.clone());
                    // let (aquamarine, aqua_handle) = aquamarine_with_backend(pool_size, None);
                    let (aquamarine, aqua_handle) = aquamarine_api();

                    let (sink, _) = mpsc::unbounded();
                    let particle_stream: BackPressuredInlet<Particle> =
                        task::block_on(particles(n));
                    let process_fut = Box::new(con.clone().process_particles(
                        particle_parallelism,
                        particle_stream,
                        aquamarine,
                        sink,
                        particle_timeout,
                    ));

                    let res = (process_fut.boxed(), finish_fut, kademlia, aqua_handle);

                    std::thread::sleep(Duration::from_secs(5));

                    println!("finished batch setup");

                    res
                },
                move |(process, finish, kad_handle, aqua_handle)| {
                    task::block_on(async move {
                        println!("start iteration");
                        let start = Instant::now();
                        let process = async_std::task::spawn(process);
                        let spawn_took = start.elapsed().as_millis();

                        let start = Instant::now();
                        finish.await;
                        let finish_took = start.elapsed().as_millis();

                        let start = Instant::now();
                        kad_handle.cancel().await;
                        aqua_handle.cancel().await;
                        process.cancel().await;
                        let cancel_took = start.elapsed().as_millis();

                        println!(
                            "spawn {} ms; finish {} ms; cancel {} ms;",
                            spawn_took, finish_took, cancel_took
                        )
                    })
                },
                BatchSize::LargeInput,
            )
        });
    });

    std::thread::sleep(std::time::Duration::from_secs(15));

    let subscriber = downcaster.downcast(&dispatcher).expect("downcast failed");
    subscriber.force_synchronize();

    subscriber.with_histograms(|hs| {
        println!("histogram: {}", hs.len());

        for (span, events) in hs.iter_mut() {
            for (event, histogram) in events.iter_mut() {
                //

                println!("span {} event {}:", span, event);
                println!(
                    "mean: {:.1}µs, p50: {}µs, p90: {}µs, p99: {}µs, p999: {}µs, max: {}µs",
                    histogram.mean() / 1000.0,
                    histogram.value_at_quantile(0.5) / 1_000,
                    histogram.value_at_quantile(0.9) / 1_000,
                    histogram.value_at_quantile(0.99) / 1_000,
                    histogram.value_at_quantile(0.999) / 1_000,
                    histogram.max() / 1_000,
                );
            }
        }

        // for v in break_once(
        //     h.iter_linear(25_000).skip_while(|v| v.quantile() < 0.01),
        //     |v| v.quantile() > 0.95,
        // ) {
        //     println!(
        //         "{:4}µs | {:40} | {:4.1}th %-ile",
        //         (v.value_iterated_to() + 1) / 1_000,
        //         "*".repeat(
        //             (v.count_since_last_iteration() as f64 * 40.0 / h.len() as f64).ceil() as usize
        //         ),
        //         v.percentile(),
        //     );
        // }
    });
}

//     }
// }

criterion_group!(
    benches,
    thousand_particles_bench,
    particle_throughput_bench,
    thousand_particles_with_aquamarine_bench,
    particle_throughput_with_delay_bench,
    particle_throughput_with_kad_bench
);
criterion_main!(benches);
