use std::{collections::HashMap, error::Error};
use futures::StreamExt;
use libp2p::{gossipsub, kad::{self, store::MemoryStore}, mdns, noise, swarm::{ NetworkBehaviour, SwarmEvent}, tcp, yamux};
use tokio::{io::{self, AsyncBufReadExt}, select};



#[derive(NetworkBehaviour)]
struct Behaviour {
    mdns: mdns::tokio::Behaviour,
    gossipsub: gossipsub::Behaviour,
    kad: kad::Behaviour<MemoryStore>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::new(), 
            noise::Config::new, 
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| {
            let kad = kad::Behaviour::new(
                key.public().to_peer_id(),
                MemoryStore::new(key.public().to_peer_id())
            );
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()), 
                gossipsub::Config::default(),
            )?;
            let mdns = mdns::Behaviour::new(
                mdns::Config::default(), 
                key.public().to_peer_id(),
            )?;
            Ok(Behaviour{mdns, gossipsub, kad})
        })?
        .build();
    let topic = gossipsub::IdentTopic::new("start");
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    swarm.behaviour_mut().kad.set_mode(Some(kad::Mode::Server));
    
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    
    let mut stdin = io::BufReader::new(io::stdin()).lines();
    
    println!("Write your name");
    let name = stdin.next_line().await.unwrap().unwrap();
    let record = kad::Record { 
        key: kad::RecordKey::new(&swarm.local_peer_id().to_base58()), 
        value: name.into(), 
        publisher: None, 
        expires: None 
    };
    swarm.behaviour_mut().kad.put_record(record, kad::Quorum::One).expect("The error in saving the record");

    let mut msg_store: HashMap<String, Vec<u8>> = HashMap::new();
    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), line.as_bytes()) {
                    println!("Error in the topic: {e:?}");
                }
            },
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr {address, ..} => {
                        println!("Listening on the address: {address}");
                    },
                    SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers)))  => {
                        for (peer_id, addr) in peers {
                            println!("Connected to the peer: {:?}", peer_id);
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                        }
                    },

                    SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message { 
                        propagation_source: peer_id, 
                        message_id: _id, 
                        message 
                    })) => {
                        let record_key = kad::RecordKey::new(&peer_id.to_base58());
                        msg_store.insert(peer_id.to_string(), message.data.clone());
                        swarm.behaviour_mut().kad.get_record(record_key);
                    },
                    SwarmEvent::Behaviour(BehaviourEvent::Kad(kad::Event::OutboundQueryProgressed { id: _, result, stats: _, step: _ })) => {
                        match result {
                            kad::QueryResult::GetRecord(Ok(
                                kad::GetRecordOk::FoundRecord(kad::PeerRecord {peer: _,record}))) => {
                                    let msg_value = String::from_utf8(record.value).unwrap_or_else(|_|"not found".to_string());
                                    let msg_key = String::from_utf8_lossy(record.key.as_ref()).to_string();
                                    if let Some(msg) = msg_store.remove(&msg_key) {
                                        println!("{msg_value}: {}", String::from_utf8_lossy(&msg));
                                    }
                                }
                            _ => {}
                        }
                    },
                    _ => {}
                }
            }
        }
    }
}