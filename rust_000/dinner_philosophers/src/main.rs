use std::thread;
use std::time::Duration;
use std::sync::{Arc, Mutex};

struct Philosopher {
    name: String,
    left: usize,
    right: usize,
}
 
impl Philosopher {
    fn new(name: &str, left: usize, right: usize) -> Philosopher {
        Philosopher {
            name: name.to_string(),
            left,
            right,
        }
    }

    fn eat(&self, table: &Table){
        let _left = table.forks[self.left].lock().unwrap();
        thread::sleep(Duration::from_millis(200));
        let _right = table.forks[self.right].lock().unwrap();

        println!("{} is eating", self.name);
        thread::sleep(Duration::from_millis(2000));
        println!("{} is done eating", self.name);
    }
}

struct Table {
    forks: Vec<Mutex<()>>,
}

fn main() {
    let table = Arc::new(Table {
        forks: vec![
            Mutex::new(()),
            Mutex::new(()),
            Mutex::new(()),
            Mutex::new(()),
            Mutex::new(()),
        ],
    }); 
    
    let philosophers = vec![
        Philosopher::new("苏格拉底", 0, 1),
        Philosopher::new("柏拉图", 1, 2),
        Philosopher::new("亚里士多德", 2, 3),
        Philosopher::new("老子", 3, 4),
        Philosopher::new("孔子", 0, 4)
    ];

    let handles: Vec<_> = philosophers.into_iter().map(|p| {
        let table = table.clone();
        thread::spawn(move || {
            p.eat(&table);
        })
    }).collect();
    
    for handle in handles {
        handle.join().unwrap();
    }
}