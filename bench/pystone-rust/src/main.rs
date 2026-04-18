use std::cell::RefCell;
use std::env;
use std::hint::black_box;
use std::process;
use std::rc::Rc;
use std::time::Instant;

const LOOPS: u64 = 50_000;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ident {
    Ident1 = 1,
    Ident2 = 2,
    Ident3 = 3,
    Ident4 = 4,
    Ident5 = 5,
}

type RecordRef = Rc<RefCell<Record>>;

#[derive(Debug)]
struct Record {
    ptr_comp: Option<RecordRef>,
    discr: Ident,
    enum_comp: Ident,
    int_comp: i64,
    string_comp: &'static str,
}

impl Record {
    fn new() -> Self {
        Self {
            ptr_comp: None,
            discr: Ident::Ident1,
            enum_comp: Ident::Ident1,
            int_comp: 0,
            string_comp: "",
        }
    }

    fn copy_shallow(&self) -> Self {
        Self {
            ptr_comp: self.ptr_comp.clone(),
            discr: self.discr,
            enum_comp: self.enum_comp,
            int_comp: self.int_comp,
            string_comp: self.string_comp,
        }
    }
}

struct PystoneState {
    int_glob: i64,
    bool_glob: bool,
    char1_glob: u8,
    char2_glob: u8,
    array1_glob: [i64; 51],
    array2_glob: [[i64; 51]; 51],
    ptr_glb: Option<RecordRef>,
    ptr_glb_next: Option<RecordRef>,
    int_sink: i64,
    float_sink: f64,
    bool_sink: bool,
}

impl PystoneState {
    fn new() -> Self {
        Self {
            int_glob: 0,
            bool_glob: false,
            char1_glob: 0,
            char2_glob: 0,
            array1_glob: [0; 51],
            array2_glob: [[0; 51]; 51],
            ptr_glb: None,
            ptr_glb_next: None,
            int_sink: 0,
            float_sink: 0.0,
            bool_sink: false,
        }
    }

    fn checksum(&self) -> i64 {
        let mut value = self.int_glob;
        value += i64::from(self.bool_glob);
        value += i64::from(self.bool_sink);
        value += self.char1_glob as i64;
        value += self.char2_glob as i64;
        value += self.array1_glob[8];
        value += self.array2_glob[8][7];
        value += self.int_sink;
        value += self.float_sink as i64;
        if let Some(ptr) = &self.ptr_glb {
            let record = ptr.borrow();
            value += record.int_comp;
            value += record.discr as i64;
            value += record.enum_comp as i64;
            value += record.string_comp.len() as i64;
            if let Some(next) = &record.ptr_comp {
                value += next.borrow().int_comp;
            }
        }
        value
    }
}

fn main() {
    let loops = parse_loops();
    let (benchtime_ns, loops_per_ns, checksum) = pystones(loops);
    println!(
        "{} {} loops/s checksum={}",
        benchtime_ns,
        (loops_per_ns * 1e9) as u64,
        checksum
    );
}

fn parse_loops() -> u64 {
    let args = env::args().collect::<Vec<_>>();
    match args.as_slice() {
        [_] => LOOPS,
        [_, loops] => match loops.parse::<u64>() {
            Ok(value) => value,
            Err(_) => {
                eprintln!(
                    "Invalid argument {loops:?}; usage: {} [number_of_loops]",
                    args[0]
                );
                process::exit(100);
            }
        },
        _ => {
            eprintln!(
                "{} arguments are too many; usage: {} [number_of_loops]",
                args.len() - 1,
                args[0]
            );
            process::exit(100);
        }
    }
}

fn pystones(loops: u64) -> (u128, f64, i64) {
    proc0(loops)
}

fn proc0(loops: u64) -> (u128, f64, i64) {
    let mut state = PystoneState::new();

    let start = Instant::now();
    for _ in 0..black_box(loops) {}
    let nulltime_ns = start.elapsed().as_nanos();

    let ptr_glb_next = Rc::new(RefCell::new(Record::new()));
    let ptr_glb = Rc::new(RefCell::new(Record::new()));
    {
        let mut record = ptr_glb.borrow_mut();
        record.ptr_comp = Some(ptr_glb_next.clone());
        record.discr = Ident::Ident1;
        record.enum_comp = Ident::Ident3;
        record.int_comp = 40;
        record.string_comp = "DHRYSTONE PROGRAM, SOME STRING";
    }
    state.ptr_glb_next = Some(ptr_glb_next);
    state.ptr_glb = Some(ptr_glb);

    let string1_loc = "DHRYSTONE PROGRAM, 1'ST STRING";
    state.array2_glob[8][7] = 10;

    let start = Instant::now();
    for _ in 0..loops {
        proc5(&mut state);
        proc4(&mut state);
        let mut int_loc1 = 2_i64;
        let int_loc2 = 3_i64;
        let string2_loc = "DHRYSTONE PROGRAM, 2'ND STRING";
        let mut enum_loc = Ident::Ident2;

        state.bool_glob = !func2(string1_loc, string2_loc);
        let mut int_loc3 = 0_i64;
        while int_loc1 < int_loc2 {
            state.int_sink = 5 * int_loc1 - int_loc2;
            int_loc3 = proc7(int_loc1, int_loc2);
            int_loc1 += 1;
        }
        proc8(&mut state, int_loc1, int_loc3);

        let ptr_glb = state
            .ptr_glb
            .as_ref()
            .expect("PtrGlb should be initialized")
            .clone();
        state.ptr_glb = Some(proc1(&mut state, ptr_glb));

        let mut char_index = b'A';
        while char_index <= state.char2_glob {
            if enum_loc == func1(char_index, b'C') {
                enum_loc = proc6(&state, Ident::Ident1);
            }
            char_index += 1;
        }

        int_loc3 = int_loc2 * int_loc1;
        let int_loc2_float = int_loc3 as f64 / int_loc1 as f64;
        state.float_sink = 7.0 * (int_loc3 as f64 - int_loc2_float) - int_loc1 as f64;
        int_loc1 = proc2(&state, int_loc1);
        black_box(int_loc1);
    }

    let elapsed_ns = start.elapsed().as_nanos();
    let benchtime_ns = elapsed_ns.saturating_sub(nulltime_ns);
    let loops_per_ns = if benchtime_ns == 0 {
        0.0
    } else {
        loops as f64 / benchtime_ns as f64
    };
    let checksum = black_box(state.checksum());
    (benchtime_ns, loops_per_ns, checksum)
}

fn proc1(state: &mut PystoneState, ptr_par_in: RecordRef) -> RecordRef {
    let next_record = {
        let ptr_glb = state
            .ptr_glb
            .as_ref()
            .expect("PtrGlb should exist")
            .borrow();
        Rc::new(RefCell::new(ptr_glb.copy_shallow()))
    };
    ptr_par_in.borrow_mut().ptr_comp = Some(next_record.clone());
    ptr_par_in.borrow_mut().int_comp = 5;

    let ptr_par_int = ptr_par_in.borrow().int_comp;
    next_record.borrow_mut().int_comp = ptr_par_int;
    next_record.borrow_mut().ptr_comp = ptr_par_in.borrow().ptr_comp.clone();
    let next_ptr = next_record.borrow().ptr_comp.clone();
    next_record.borrow_mut().ptr_comp = proc3(state, next_ptr);

    let mut result = ptr_par_in.clone();
    if next_record.borrow().discr == Ident::Ident1 {
        next_record.borrow_mut().int_comp = 6;
        let enum_comp = ptr_par_in.borrow().enum_comp;
        next_record.borrow_mut().enum_comp = proc6(state, enum_comp);
        next_record.borrow_mut().ptr_comp = state
            .ptr_glb
            .as_ref()
            .expect("PtrGlb should exist")
            .borrow()
            .ptr_comp
            .clone();
        let int_comp = next_record.borrow().int_comp;
        next_record.borrow_mut().int_comp = proc7(int_comp, 10);
    } else {
        result = Rc::new(RefCell::new(next_record.borrow().copy_shallow()));
    }
    next_record.borrow_mut().ptr_comp = None;
    result
}

fn proc2(state: &PystoneState, int_par_io: i64) -> i64 {
    let mut int_par_io = int_par_io;
    let mut int_loc = int_par_io + 10;
    loop {
        let enum_loc;
        if state.char1_glob == b'A' {
            int_loc -= 1;
            int_par_io = int_loc - state.int_glob;
            enum_loc = Ident::Ident1;
        } else {
            enum_loc = Ident::Ident2;
        }
        if enum_loc == Ident::Ident1 {
            break;
        }
    }
    int_par_io
}

fn proc3(state: &mut PystoneState, mut ptr_par_out: Option<RecordRef>) -> Option<RecordRef> {
    if let Some(ptr_glb) = &state.ptr_glb {
        ptr_par_out = ptr_glb.borrow().ptr_comp.clone();
    } else {
        state.int_glob = 100;
    }
    let int_glob = state.int_glob;
    if let Some(ptr_glb) = &state.ptr_glb {
        ptr_glb.borrow_mut().int_comp = proc7(10, int_glob);
    }
    ptr_par_out
}

fn proc4(state: &mut PystoneState) {
    let bool_loc = state.char1_glob == b'A';
    state.bool_sink = bool_loc || state.bool_glob;
    state.char2_glob = b'B';
}

fn proc5(state: &mut PystoneState) {
    state.char1_glob = b'A';
    state.bool_glob = false;
}

fn proc6(state: &PystoneState, enum_par_in: Ident) -> Ident {
    let mut enum_par_out = enum_par_in;
    if !func3(enum_par_in) {
        enum_par_out = Ident::Ident4;
    }
    match enum_par_in {
        Ident::Ident1 => enum_par_out = Ident::Ident1,
        Ident::Ident2 => {
            if state.int_glob > 100 {
                enum_par_out = Ident::Ident1;
            } else {
                enum_par_out = Ident::Ident4;
            }
        }
        Ident::Ident3 => enum_par_out = Ident::Ident2,
        Ident::Ident4 => {}
        Ident::Ident5 => enum_par_out = Ident::Ident3,
    }
    enum_par_out
}

fn proc7(int_par_i1: i64, int_par_i2: i64) -> i64 {
    let int_loc = int_par_i1 + 2;
    int_par_i2 + int_loc
}

fn proc8(state: &mut PystoneState, int_par_i1: i64, int_par_i2: i64) {
    let int_loc = (int_par_i1 + 5) as usize;
    state.array1_glob[int_loc] = int_par_i2;
    state.array1_glob[int_loc + 1] = state.array1_glob[int_loc];
    state.array1_glob[int_loc + 30] = int_loc as i64;
    for int_index in int_loc..(int_loc + 2) {
        state.array2_glob[int_loc][int_index] = int_loc as i64;
    }
    state.array2_glob[int_loc][int_loc - 1] += 1;
    state.array2_glob[int_loc + 20][int_loc] = state.array1_glob[int_loc];
    state.int_glob = 5;
}

fn func1(char_par1: u8, char_par2: u8) -> Ident {
    let char_loc1 = char_par1;
    let char_loc2 = char_loc1;
    if char_loc2 != char_par2 {
        Ident::Ident1
    } else {
        Ident::Ident2
    }
}

fn func2(str_par_i1: &str, str_par_i2: &str) -> bool {
    let bytes1 = str_par_i1.as_bytes();
    let bytes2 = str_par_i2.as_bytes();
    let mut int_loc = 1_usize;
    let mut char_loc = b'\0';
    while int_loc <= 1 {
        if func1(bytes1[int_loc], bytes2[int_loc + 1]) == Ident::Ident1 {
            char_loc = b'A';
            int_loc += 1;
        }
    }
    if (b'W'..=b'Z').contains(&char_loc) {
        int_loc = 7;
    }
    if char_loc == b'X' {
        true
    } else if str_par_i1 > str_par_i2 {
        let _ = int_loc + 7;
        true
    } else {
        false
    }
}

fn func3(enum_par_in: Ident) -> bool {
    enum_par_in == Ident::Ident3
}
