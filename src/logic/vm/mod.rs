#![allow(dead_code)]

mod buildings;
mod instructions;
mod processor;
mod variables;

use std::{
    borrow::Cow,
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use thiserror::Error;

use self::{
    buildings::{Building, BuildingData},
    variables::LVar,
};
use crate::types::{Point2, Schematic, SchematicTile};

const MILLIS_PER_SEC: u64 = 1_000;
const NANOS_PER_MILLI: u32 = 1_000_000;

pub struct LogicVM<'a> {
    /// Sorted with all processors first, then all other buildings.
    buildings: Vec<Building>,
    buildings_map: HashMap<Point2, usize>,
    total_processors: usize,
    running_processors: Rc<Cell<usize>>,
    time: Rc<Cell<f64>>,
    globals: Cow<'a, HashMap<String, LVar>>,
}

impl<'a> LogicVM<'a> {
    pub fn new() -> Self {
        Self::new_with_globals(Cow::Owned(LVar::create_globals()))
    }

    pub fn new_with_globals(globals: Cow<'a, HashMap<String, LVar>>) -> Self {
        Self {
            buildings: Vec::new(),
            buildings_map: HashMap::new(),
            total_processors: 0,
            running_processors: Rc::new(Cell::new(0)),
            time: Rc::new(Cell::new(0.)),
            globals,
        }
    }

    pub fn from_schematic(schematic: &Schematic) -> VMLoadResult<Self> {
        Self::from_schematic_tiles(schematic.tiles())
    }

    pub fn from_schematic_tiles(tiles: &[SchematicTile]) -> VMLoadResult<Self> {
        let mut vm = Self::new();
        vm.add_schematic_tiles(tiles)?;
        Ok(vm)
    }

    pub fn add_building(&mut self, building: Building) {
        let position = building.position;
        let size = building.block.size;

        let index = if let BuildingData::Processor(processor) = &*building.data.borrow() {
            if processor.state.enabled() {
                self.running_processors.update(|n| n + 1);
            }
            self.total_processors += 1;
            self.total_processors - 1
        } else {
            self.buildings.len()
        };

        self.buildings.insert(index, building);

        for x in position.x..position.x + size {
            for y in position.y..position.y + size {
                self.buildings_map.insert(Point2 { x, y }, index);
            }
        }
    }

    pub fn add_buildings<T>(&mut self, buildings: T)
    where
        T: IntoIterator<Item = Building>,
    {
        for building in buildings.into_iter() {
            self.add_building(building);
        }
    }

    pub fn add_schematic_tile(&mut self, tile: &SchematicTile) -> VMLoadResult<()> {
        let building = Building::from_schematic_tile(tile, self)?;
        self.add_building(building);
        Ok(())
    }

    pub fn add_schematic_tiles(&mut self, tiles: &[SchematicTile]) -> VMLoadResult<()> {
        for tile in tiles {
            self.add_schematic_tile(tile)?;
        }
        Ok(())
    }

    pub fn building(&self, position: Point2) -> Option<&Building> {
        self.buildings_map
            .get(&position)
            .map(|&i| &self.buildings[i])
    }

    /// Run the simulation until all processors halt, or until a number of ticks are finished.
    /// Returns true if all processors halted, or false if the tick limit was reached.
    pub fn run(&mut self, max_ticks: Option<usize>) -> bool {
        let mut now = Instant::now();
        let mut tick = 0;

        loop {
            self.do_tick(now.elapsed());
            now = Instant::now();

            if self.running_processors.get() == 0 {
                // all processors finished, return true
                return true;
            }

            if let Some(max_ticks) = max_ticks {
                tick += 1;
                if tick >= max_ticks {
                    // hit tick limit, return false
                    return false;
                }
            }
        }
    }

    /// Execute one tick of the simulation.
    ///
    /// Note: negative delta values will be ignored.
    pub fn do_tick(&mut self, delta: Duration) {
        // never move time backwards
        let time = self.time.get() + duration_millis_f64(delta).max(0.);
        self.time.set(time);

        for processor in self.buildings.iter().take(self.total_processors) {
            processor
                .data
                .borrow_mut()
                .unwrap_processor_mut()
                .do_tick(self, time);
        }
    }
}

impl Default for LogicVM<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn duration_millis_f64(d: Duration) -> f64 {
    // reimplementation of the unstable function as_millis_f64
    (d.as_secs() as f64) * (MILLIS_PER_SEC as f64)
        + (d.subsec_nanos() as f64) / (NANOS_PER_MILLI as f64)
}

pub type VMLoadResult<T> = Result<T, VMLoadError>;

#[derive(Error, Debug)]
pub enum VMLoadError {
    #[error("unknown block type: {0}")]
    UnknownBlockType(String),

    #[error("expected {want} block type but got {got}")]
    BadBlockType { want: String, got: String },

    #[error("failed to decode processor config")]
    BadProcessorConfig(#[from] binrw::Error),

    #[error("failed to parse processor code: {0}")]
    BadProcessorCode(String),

    #[error("tried to place multiple blocks at {0}")]
    Overlap(Point2),
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use binrw::BinWrite;
    use itertools::Itertools;
    use pretty_assertions::assert_eq;

    use crate::{
        logic::vm::{
            buildings::{HYPER_PROCESSOR, MICRO_PROCESSOR},
            variables::{Content, LValue, LVar},
        },
        types::{Object, PackedPoint2, ProcessorConfig, Team, colors::COLORS},
    };

    use super::{buildings::WORLD_PROCESSOR, processor::Processor, *};

    fn single_processor_vm<'a>(name: &str, code: &str) -> LogicVM<'a> {
        let mut vm = LogicVM::new();
        vm.add_building(
            Building::from_processor_config(
                name,
                Point2::new(0, 0),
                &ProcessorConfig::from_code(code),
                &vm,
            )
            .unwrap(),
        );
        vm
    }

    fn single_processor_vm_with_globals<'a>(
        name: &str,
        code: &str,
        globals: &'a HashMap<String, LVar>,
    ) -> LogicVM<'a> {
        let mut vm = LogicVM::new_with_globals(Cow::Borrowed(globals));
        vm.add_building(
            Building::from_processor_config(
                name,
                Point2::new(0, 0),
                &ProcessorConfig::from_code(code),
                &vm,
            )
            .unwrap(),
        );
        vm
    }

    fn run(vm: &mut LogicVM, max_ticks: usize, want: bool) {
        assert_eq!(
            vm.run(Some(max_ticks)),
            want,
            "VM took {cmp}{max_ticks} tick{s} to finish",
            cmp = if want { ">=" } else { "<" },
            s = if max_ticks == 1 { "" } else { "s" },
        );
    }

    fn with_processor(vm: &mut LogicVM, idx: usize, f: impl FnOnce(&mut Processor)) {
        f(vm.buildings[idx].data.borrow_mut().unwrap_processor_mut())
    }

    fn take_processor(vm: &mut LogicVM, idx: usize) -> Processor {
        vm.buildings[idx]
            .data
            .replace(BuildingData::Unknown {
                config: Object::Null,
            })
            .into_processor()
    }

    #[test]
    fn test_empty() {
        let mut vm = LogicVM::from_schematic_tiles(&[]).unwrap();
        run(&mut vm, 1, true);
    }

    #[test]
    fn test_from_schematic_tiles() {
        let mut vm = LogicVM::from_schematic_tiles(&[SchematicTile {
            block: "logic-processor".into(),
            position: PackedPoint2 { x: 0, y: 0 },
            config: {
                let mut cur = Cursor::new(Vec::new());
                ProcessorConfig::from_code("stop").write(&mut cur).unwrap();
                cur.into_inner().into()
            },
            rotation: 0,
        }])
        .unwrap();

        run(&mut vm, 1, true);

        let processor = take_processor(&mut vm, 0);
        assert_eq!(processor.state.counter, 0);
        assert!(processor.state.stopped());
    }

    #[test]
    fn test_max_ticks() {
        let mut vm = single_processor_vm(
            MICRO_PROCESSOR,
            "
            noop
            noop
            stop
            ",
        );

        run(&mut vm, 1, false);

        let processor = take_processor(&mut vm, 0);
        assert_eq!(processor.state.counter, 2);
        assert!(!processor.state.stopped());
    }

    #[test]
    fn test_print() {
        let mut vm = single_processor_vm(
            HYPER_PROCESSOR,
            r#"
            print "foo"
            print "bar\n"
            print 10
            print "\n"
            print 1.5
            print null
            print foo
            print "♥"
            stop
            "#,
        );

        run(&mut vm, 1, true);

        let processor = take_processor(&mut vm, 0);
        assert_eq!(
            processor.state.decode_printbuffer(),
            "foobar\n10\n1.5nullnull♥"
        );
        assert_eq!(
            processor.state.printbuffer,
            "foobar\n10\n1.5nullnull"
                .bytes()
                .map(|b| b as u16)
                .chain([0x2665])
                .collect_vec()
        );
    }

    #[test]
    fn test_end() {
        let mut vm = single_processor_vm(
            MICRO_PROCESSOR,
            "
            print 1
            end
            print 2
            ",
        );

        run(&mut vm, 2, false);

        let processor = take_processor(&mut vm, 0);
        assert_eq!(processor.state.counter, 3);
        assert!(!processor.state.stopped());
        assert_eq!(processor.state.decode_printbuffer(), "11");
    }

    #[test]
    fn test_set() {
        let mut vm = single_processor_vm(
            MICRO_PROCESSOR,
            r#"
            set foo 1
            noop
            set foo 2
            set @counter 6
            stop
            stop
            set @counter null
            set @counter "foo"
            set @ipt 10
            set true 0
            set pi @pi
            set pi_fancy π
            set e @e
            noop
            set a 1e308
            set b 1e309
            set c -1e308
            set d -1e309
            "#,
        );

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.variables["foo"].get(&p.state), LValue::Null);
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.variables["foo"].get(&p.state), LValue::Number(1.));
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.variables["foo"].get(&p.state), LValue::Number(2.));
            assert_eq!(p.state.counter, 6);
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.counter, 8);
        });

        vm.do_tick(Duration::ZERO);
        vm.do_tick(Duration::ZERO);
        vm.do_tick(Duration::ZERO);

        assert_eq!(vm.globals["@ipt"], LVar::Ipt);
        assert_eq!(vm.globals["true"].try_get().unwrap(), LValue::Number(1.));

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.variables.get("@ipt"), None);
            assert_eq!(p.state.variables.get("true"), None);
            assert_eq!(
                p.state.variables["pi"].get(&p.state),
                LValue::Number(variables::PI.into())
            );
            assert_eq!(
                p.state.variables["pi_fancy"].get(&p.state),
                LValue::Number(variables::PI.into())
            );
            assert_eq!(
                p.state.variables["e"].get(&p.state),
                LValue::Number(variables::E.into())
            );
        });

        vm.do_tick(Duration::ZERO);
        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.variables["a"].get(&p.state), LValue::Number(1e308));
            assert_eq!(p.state.variables["b"].get(&p.state), LValue::Null);
            assert_eq!(p.state.variables["c"].get(&p.state), LValue::Number(-1e308));
            assert_eq!(p.state.variables["d"].get(&p.state), LValue::Null);
        });
    }

    #[test]
    fn test_setrate() {
        let mut vm = single_processor_vm(
            WORLD_PROCESSOR,
            "
            noop
            setrate 0
            stop
            setrate 10
            stop
            setrate 1
            stop
            setrate 1001
            stop
            setrate 500
            stop
            setrate 1000
            stop
            setrate 5.5
            stop
            ",
        );

        for ipt in [8, 1, 10, 1, 1000, 500, 1000, 5] {
            with_processor(&mut vm, 0, |p| {
                assert_eq!(
                    p.state.ipt, ipt,
                    "incorrect ipt at counter {}",
                    p.state.counter
                );
                p.state.counter += 1;
                p.state.set_stopped(false);
            });

            vm.do_tick(Duration::ZERO);
        }
    }

    #[test]
    fn test_setrate_unpriv() {
        let mut vm = single_processor_vm(MICRO_PROCESSOR, "setrate 10; stop");
        run(&mut vm, 1, true);
        let processor = take_processor(&mut vm, 0);
        assert_eq!(processor.state.ipt, 2);
    }

    const CONDITION_TESTS: &[(&str, &str, &str, bool)] = &[
        // equal
        ("equal", "0", "0", true),
        ("equal", "0", "null", true),
        ("equal", "1", r#""""#, true),
        ("equal", "1", r#""foo""#, true),
        ("equal", r#""""#, r#""""#, true),
        ("equal", r#""abc""#, r#""abc""#, true),
        ("equal", "null", "null", true),
        ("equal", "0", "0.0000009", true),
        ("equal", "@pi", "3.1415927", true),
        ("equal", "π", "3.1415927", true),
        ("equal", "@e", "2.7182818", true),
        ("equal", "0", "0.000001", false),
        ("equal", "0", "1", false),
        ("equal", "1", "null", false),
        ("equal", r#""abc""#, r#""def""#, false),
        // notEqual
        ("notEqual", "0", "0", false),
        ("notEqual", "0", "null", false),
        ("notEqual", "null", "null", false),
        ("notEqual", "0", "0.0000009", false),
        ("notEqual", "0", "0.000001", true),
        ("notEqual", "0", "1", true),
        ("notEqual", "1", "null", true),
        // lessThan
        ("lessThan", "0", "1", true),
        ("lessThan", "0", "0", false),
        ("lessThan", "1", "0", false),
        // lessThanEq
        ("lessThanEq", "0", "1", true),
        ("lessThanEq", "0", "0", true),
        ("lessThanEq", "1", "0", false),
        // greaterThan
        ("greaterThan", "0", "1", false),
        ("greaterThan", "0", "0", false),
        ("greaterThan", "1", "0", true),
        // greaterThanEq
        ("greaterThanEq", "0", "1", false),
        ("greaterThanEq", "0", "0", true),
        ("greaterThanEq", "1", "0", true),
        // strictEqual
        ("strictEqual", "0", "0", true),
        ("strictEqual", "0.5", "0.5", true),
        ("strictEqual", "null", "null", true),
        ("strictEqual", r#""""#, r#""""#, true),
        ("strictEqual", r#""abc""#, r#""abc""#, true),
        ("strictEqual", "0", "null", false),
        ("strictEqual", "1", r#""""#, false),
        ("strictEqual", "1", r#""foo""#, false),
        ("strictEqual", r#""abc""#, r#""def""#, false),
        ("strictEqual", "0", "0.0000009", false),
        ("strictEqual", "0", "0.000001", false),
        ("strictEqual", "0", "1", false),
        ("strictEqual", "1", "null", false),
        // always
        ("always", "0", "0", true),
        ("always", "0", "1", true),
        ("always", "1", "0", true),
        ("always", "1", "1", true),
    ];

    #[test]
    fn test_jump() {
        let mut code = r#"
        setrate 1000

        # detect reentry
        set source "reentered init"
        jump oops notEqual canary null
        set source null

        set canary 0xdeadbeef

        # test 'always' manually

        jump test_always_0 always
            set canary "test_always_0 not taken: always"
            stop
        test_always_0:

        jump test_always_1 always 1
            set canary "test_always_1 not taken: always 1"
            stop
        test_always_1:
        "#
        .to_string();

        for (i, &(cond, x, y, want_jump)) in CONDITION_TESTS.iter().enumerate() {
            let err = format!("{cond} {x} {y}").replace('"', "'");
            if want_jump {
                code.push_str(&format!(
                    "
                    jump test{i}_0 {cond} {x} {y}
                        set canary \"test{i}_0 not taken: {err}\"
                        stop
                    test{i}_0:

                    set x {x}
                    set y {y}
                    jump test{i}_1 {cond} x y
                        set canary \"test{i}_1 not taken: {err}\"
                        stop
                    test{i}_1:
                    "
                ));
            } else {
                code.push_str(&format!(
                    "
                    set source \"test{i}_0 taken: {err}\"
                    jump oops {cond} {x} {y}
                    set x {x}
                    set y {y}
                    set source \"test{i}_1 taken: {err}\"
                    jump oops {cond} x y
                    set source null
                    "
                ));
            }
        }

        code += "
        stop

        oops:
        set canary source
        stop
        ";

        let mut vm = single_processor_vm(WORLD_PROCESSOR, &code);

        run(&mut vm, 2, true);

        let state = take_processor(&mut vm, 0).state;
        assert_eq!(
            state.variables["canary"].get(&state),
            LValue::Number(0xdeadbeefu64 as f64)
        );
    }

    #[test]
    fn test_wait() {
        let mut vm = single_processor_vm(
            HYPER_PROCESSOR,
            "
            print 1
            wait -1

            print 2
            wait 0

            print 3
            wait 1e-5

            print 4
            wait 1

            print 5
            stop
            ",
        );

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.decode_printbuffer(), "123");
        });

        vm.do_tick(Duration::from_secs_f64(1. / 60.));

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.decode_printbuffer(), "1234");
        });

        vm.do_tick(Duration::from_millis(500));

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.decode_printbuffer(), "1234");
        });

        vm.do_tick(Duration::from_millis(500));

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.decode_printbuffer(), "12345");
        });
    }

    #[test]
    fn test_printchar() {
        // TODO: test full range (requires op add)
        let mut vm = single_processor_vm(
            HYPER_PROCESSOR,
            "
            printchar 0
            printchar 10
            printchar 0x41
            printchar 0xc0
            printchar 0x2665
            printchar 0xd799
            printchar 0xd800
            printchar 0xdfff
            printchar 0x8000
            printchar 0xffff
            printchar 0x10000
            printchar 0x10001
            stop
            ",
        );

        run(&mut vm, 1, true);

        let state = take_processor(&mut vm, 0).state;
        assert_eq!(
            state.printbuffer,
            &[
                0, 10, 0x41, 0xc0, 0x2665, 0xd799, 0xd800, 0xdfff, 0x8000, 0xffff, 0, 1
            ]
        );
    }

    #[test]
    fn test_format() {
        let mut vm = single_processor_vm(
            MICRO_PROCESSOR,
            r#"
            print "{0} {1} {/} {9} {:} {10} {0}"
            noop

            format 4
            noop

            format "abcde"
            noop

            format "aa"
            noop

            format ""
            noop

            format "ignored"
            stop
            "#,
        );

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(
                p.state.decode_printbuffer(),
                r#"{0} {1} {/} {9} {:} {10} {0}"#
            );
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(
                p.state.decode_printbuffer(),
                r#"4 {1} {/} {9} {:} {10} {0}"#
            );
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(
                p.state.decode_printbuffer(),
                r#"4 {1} {/} {9} {:} {10} abcde"#
            );
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(
                p.state.decode_printbuffer(),
                r#"4 aa {/} {9} {:} {10} abcde"#
            );
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.decode_printbuffer(), r#"4 aa {/}  {:} {10} abcde"#);
        });

        vm.do_tick(Duration::ZERO);

        with_processor(&mut vm, 0, |p| {
            assert_eq!(p.state.decode_printbuffer(), r#"4 aa {/}  {:} {10} abcde"#);
        });
    }

    #[test]
    fn test_pack_unpack_color() {
        let mut vm = single_processor_vm(
            HYPER_PROCESSOR,
            "
            packcolor packed1 0 0.5 0.75 1
            unpackcolor r1 g1 b1 a1 packed1
            unpackcolor r2 g2 b2 a2 %[royal]
            packcolor packed2 r2 g2 b2 a2
            stop
            ",
        );

        run(&mut vm, 1, true);

        let state = take_processor(&mut vm, 0).state;

        assert_eq!(
            state.variables["packed1"].get(&state),
            LValue::Number(f64::from_bits(0x00_7f_bf_ffu64))
        );

        assert_eq!(state.variables["r1"].get(&state), LValue::Number(0.));
        assert_eq!(
            state.variables["g1"].get(&state),
            LValue::Number(127. / 255.)
        );
        assert_eq!(
            state.variables["b1"].get(&state),
            LValue::Number(191. / 255.)
        );
        assert_eq!(state.variables["a1"].get(&state), LValue::Number(1.));

        assert_eq!(
            state.variables["r2"].get(&state),
            LValue::Number((0x41 as f64) / 255.)
        );
        assert_eq!(
            state.variables["g2"].get(&state),
            LValue::Number((0x69 as f64) / 255.)
        );
        assert_eq!(
            state.variables["b2"].get(&state),
            LValue::Number((0xe1 as f64) / 255.)
        );
        assert_eq!(
            state.variables["a2"].get(&state),
            LValue::Number((0xff as f64) / 255.)
        );

        assert_eq!(
            state.variables["packed2"].get(&state),
            LValue::Number(COLORS["royal"])
        );
    }

    #[test]
    fn test_select() {
        let globals = LVar::create_globals();

        for &(cond, x, y, want_true) in CONDITION_TESTS {
            let mut vm = single_processor_vm_with_globals(
                HYPER_PROCESSOR,
                &format!(
                    "
                    set x {x}
                    set y {y}
                    set if_true 0xdeadbeef
                    set if_false 0xbabecafe
                    select got1 {cond} x y if_true if_false
                    select got2 {cond} {x} {y} 0xdeadbeef 0xbabecafe
                    stop
                    "
                ),
                &globals,
            );

            run(&mut vm, 1, true);

            let state = take_processor(&mut vm, 0).state;
            let want_value = if want_true {
                0xdeadbeefu64
            } else {
                0xbabecafeu64
            }
            .into();
            assert_eq!(
                state.variables["got1"].get(&state),
                want_value,
                "{cond} {x} {y} (variables)"
            );
            assert_eq!(
                state.variables["got2"].get(&state),
                want_value,
                "{cond} {x} {y} (constants)"
            );
        }
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_op_unary() {
        let globals = LVar::create_globals();

        for (op, x, want, epsilon) in [
            // not
            ("not", "0b00", (-1).into(), None),
            ("not", "0b01", (-2).into(), None),
            ("not", "0b10", (-3).into(), None),
            ("not", "0b11", (-4).into(), None),
            ("not", "-1", 0.into(), None),
            ("not", "-2", 1.into(), None),
            ("not", "-3", 2.into(), None),
            ("not", "-4", 3.into(), None),
            // abs
            ("abs", "0", 0.into(), None),
            ("abs", "-0", 0.into(), None),
            ("abs", "1", 1.into(), None),
            ("abs", "-1", 1.into(), None),
            ("abs", "1e308", 1e308.into(), None),
            ("abs", "1e309", 0.into(), None),
            ("abs", "-1e308", 1e308.into(), None),
            ("abs", "-1e309", 0.into(), None),
            // sign
            ("sign", "0", 0.into(), None),
            ("sign", "-0", 0.into(), None),
            ("sign", "1", 1.into(), None),
            ("sign", "-1", (-1).into(), None),
            ("sign", "1e308", 1.into(), None),
            ("sign", "1e309", 0.into(), None),
            ("sign", "-1e308", (-1).into(), None),
            ("sign", "-1e309", 0.into(), None),
            // log
            ("log", "-1", LValue::Null, None),
            ("log", "0", LValue::Null, None),
            ("log", "@e", 0.99999996963214.into(), None),
            ("log", "2", 0.6931471805599453.into(), None),
            // log10
            ("log10", "-1", LValue::Null, None),
            ("log10", "0", LValue::Null, None),
            ("log10", "100", 2.into(), None),
            ("log10", "101", 2.0043213737826426.into(), None),
            // floor
            ("floor", "0", 0.into(), None),
            ("floor", "1", 1.into(), None),
            ("floor", "1.5", 1.into(), None),
            ("floor", "-1", (-1).into(), None),
            ("floor", "-1.5", (-2).into(), None),
            // ceil
            ("ceil", "0", 0.into(), None),
            ("ceil", "1", 1.into(), None),
            ("ceil", "1.5", 2.into(), None),
            ("ceil", "-1", (-1).into(), None),
            ("ceil", "-1.5", (-1).into(), None),
            // round
            ("round", "0", 0.into(), None),
            ("round", "1", 1.into(), None),
            ("round", "1.1", 1.into(), None),
            ("round", "1.49", 1.into(), None),
            ("round", "1.5", 2.into(), None),
            ("round", "1.51", 2.into(), None),
            ("round", "1.9", 2.into(), None),
            ("round", "-1", (-1).into(), None),
            ("round", "-1.1", (-1).into(), None),
            ("round", "-1.49", (-1).into(), None),
            ("round", "-1.5", (-1).into(), None),
            ("round", "-1.51", (-2).into(), None),
            ("round", "-1.9", (-2).into(), None),
            // sqrt
            ("sqrt", "-1", LValue::Null, None),
            ("sqrt", "-0.25", LValue::Null, None),
            ("sqrt", "0", 0.into(), None),
            ("sqrt", "0.25", 0.5.into(), None),
            ("sqrt", "1", 1.into(), None),
            ("sqrt", "4", 2.into(), None),
            // sin
            ("sin", "-30", (-0.5).into(), Some(1e-15)),
            ("sin", "0", 0.into(), None),
            ("sin", "30", 0.5.into(), Some(1e-15)),
            ("sin", "45", 0.7071067811865476.into(), Some(1e-15)),
            ("sin", "60", 0.8660254037844386.into(), Some(1e-15)),
            ("sin", "90", 1.into(), Some(1e-15)),
            ("sin", "180", 0.into(), Some(1e-15)),
            // cos
            ("cos", "-30", 0.8660254037844387.into(), Some(1e-15)),
            ("cos", "0", 1.into(), None),
            ("cos", "30", 0.8660254037844387.into(), Some(1e-15)),
            ("cos", "45", 0.7071067811865476.into(), Some(1e-15)),
            ("cos", "60", 0.5.into(), Some(1e-15)),
            ("cos", "90", 6.123233995736766e-17.into(), Some(1e-15)),
            ("cos", "180", (-1).into(), Some(1e-15)),
            // tan
            ("tan", "0", 0.into(), None),
            ("tan", "45", 1.into(), Some(1e-15)),
            ("tan", "90", 16331239353195370i64.into(), Some(1e-15)),
            ("tan", "135", (-1).into(), Some(1e-15)),
            // asin
            ("asin", "-0.5", (-30).into(), Some(1e-13)),
            ("asin", "0", 0.into(), None),
            ("asin", "0.5", 30.into(), Some(1e-13)),
            // acos
            ("acos", "-0.5", 120.into(), Some(1e-13)),
            ("acos", "0", 90.into(), None),
            ("acos", "0.5", 60.into(), Some(1e-13)),
            // atan
            ("atan", "-0.5", (-26.56505117707799).into(), Some(1e-13)),
            ("atan", "0", 0.into(), None),
            ("atan", "0.5", 26.56505117707799.into(), Some(1e-13)),
        ] {
            let mut vm = single_processor_vm_with_globals(
                HYPER_PROCESSOR,
                &format!(
                    "
                    op {op} got {x}
                    stop
                    "
                ),
                &globals,
            );

            run(&mut vm, 1, true);

            let state = take_processor(&mut vm, 0).state;
            let got = state.variables["got"].get(&state);
            if let Some(epsilon) = epsilon {
                assert!(
                    (got.num() - want.num()).abs() <= epsilon,
                    "{op} {x} (got {got:?}, want {want:?} ± {epsilon:?})"
                );
            } else {
                assert_eq!(got, want, "{op} {x}");
            }
        }
    }

    #[test]
    fn test_op_binary() {
        let globals = LVar::create_globals();

        for (op, x, y, want) in [
            // add
            ("add", "0", "0", 0.into()),
            ("add", "0", "1", 1.into()),
            ("add", "1.5", "0.25", 1.75.into()),
            ("add", "1.0", "-2.0", (-1).into()),
            // sub
            ("sub", "3", "1", 2.into()),
            ("sub", "3", "-1", 4.into()),
            // mul
            ("mul", "1", "0", 0.into()),
            ("mul", "1", "1", 1.into()),
            ("mul", "3", "-4.5", (-13.5).into()),
            // div
            ("div", "5", "2", 2.5.into()),
            ("div", "-5", "2", (-2.5).into()),
            ("div", "5", "-2", (-2.5).into()),
            ("div", "-5", "-2", 2.5.into()),
            ("div", "1", "0", LValue::Null),
            ("div", "-1", "0", LValue::Null),
            ("div", "0", "0", LValue::Null),
            ("div", "0", "1", 0.into()),
            ("div", "0", "-1", 0.into()),
            // idiv
            ("idiv", "5", "2", 2.into()),
            ("idiv", "-5", "2", (-3).into()),
            ("idiv", "5", "-2", (-3).into()),
            ("idiv", "-5", "-2", 2.into()),
            ("idiv", "1", "0", LValue::Null),
            ("idiv", "-1", "0", LValue::Null),
            ("idiv", "0", "0", LValue::Null),
            ("idiv", "0", "1", 0.into()),
            ("idiv", "0", "-1", 0.into()),
            // mod
            ("mod", "5", "2", 1.into()),
            ("mod", "-5", "2", (-1).into()),
            ("mod", "5", "-2", 1.into()),
            ("mod", "-5", "-2", (-1).into()),
            // emod
            ("emod", "5", "2", 1.into()),
            ("emod", "-5", "2", 1.into()),
            ("emod", "5", "-2", (-1).into()),
            ("emod", "-5", "-2", (-1).into()),
            // pow
            ("pow", "3", "2", 9.into()),
            ("pow", "9", "0.5", 3.into()),
            ("pow", "16", "-0.5", 0.25.into()),
            ("pow", "-3", "2", 9.into()),
            ("pow", "-9", "0.5", LValue::Null),
            ("pow", "-16", "-0.5", LValue::Null),
            // land
            ("land", "false", "false", 0.into()),
            ("land", "false", "true", 0.into()),
            ("land", "true", "false", 0.into()),
            ("land", "true", "true", 1.into()),
            ("land", "\"foo\"", "null", 0.into()),
            ("land", "\"foo\"", "\"bar\"", 1.into()),
            // shl
            ("shl", "2", "0", 2.into()),
            ("shl", "2", "1", 4.into()),
            ("shl", "2", "62", (-9223372036854775808i64).into()),
            ("shl", "2", "63", 0.into()),
            ("shl", "2", "64", 2.into()),
            ("shl", "2", "-1", 0.into()),
            ("shl", "2", "-60", 32.into()),
            ("shl", "-2", "0", (-2).into()),
            ("shl", "-2", "1", (-4).into()),
            ("shl", "-2", "62", (-9223372036854775808i64).into()),
            ("shl", "-2", "63", 0.into()),
            ("shl", "-2", "64", (-2).into()),
            ("shl", "-2", "-1", 0.into()),
            ("shl", "-2", "-60", (-32).into()),
            // shr
            ("shr", "2", "0", 2.into()),
            ("shr", "2", "1", 1.into()),
            ("shr", "2", "62", 0.into()),
            ("shr", "2", "63", 0.into()),
            ("shr", "2", "64", 2.into()),
            ("shr", "2", "-1", 0.into()),
            ("shr", "2", "-60", 0.into()),
            ("shr", "-2", "0", (-2).into()),
            ("shr", "-2", "1", (-1).into()),
            ("shr", "-2", "62", (-1).into()),
            ("shr", "-2", "63", (-1).into()),
            ("shr", "-2", "64", (-2).into()),
            ("shr", "-2", "-1", (-1).into()),
            ("shr", "-2", "-60", (-1).into()),
            // ushr
            ("ushr", "2", "0", 2.into()),
            ("ushr", "2", "1", 1.into()),
            ("ushr", "2", "62", 0.into()),
            ("ushr", "2", "63", 0.into()),
            ("ushr", "2", "64", 2.into()),
            ("ushr", "2", "-1", 0.into()),
            ("ushr", "2", "-60", 0.into()),
            ("ushr", "-2", "0", (-2).into()),
            ("ushr", "-2", "1", 9223372036854775807i64.into()),
            ("ushr", "-2", "62", 3.into()),
            ("ushr", "-2", "63", 1.into()),
            ("ushr", "-2", "64", (-2).into()),
            ("ushr", "-2", "-1", 1.into()),
            ("ushr", "-2", "-60", 1152921504606846976i64.into()),
            // or
            ("or", "0b10", "0b10", 0b10.into()),
            ("or", "0b10", "0b11", 0b11.into()),
            ("or", "0b11", "0b10", 0b11.into()),
            ("or", "0b11", "0b11", 0b11.into()),
            ("or", "-1", "0", (-1).into()),
            ("or", "-1", "1", (-1).into()),
            // and
            ("and", "0b10", "0b10", 0b10.into()),
            ("and", "0b10", "0b11", 0b10.into()),
            ("and", "0b11", "0b10", 0b10.into()),
            ("and", "0b11", "0b11", 0b11.into()),
            ("and", "-1", "0", 0.into()),
            ("and", "-1", "1", 1.into()),
            // xor
            ("xor", "0b10", "0b10", 0b00.into()),
            ("xor", "0b10", "0b11", 0b01.into()),
            ("xor", "0b11", "0b10", 0b01.into()),
            ("xor", "0b11", "0b11", 0b00.into()),
            ("xor", "-1", "0", (-1).into()),
            ("xor", "-1", "1", (-2).into()),
            // max
            ("max", "-1", "1", 1.into()),
            ("max", "1", "-1", 1.into()),
            ("max", "1", "2", 2.into()),
            ("max", "2", "1", 2.into()),
            // min
            ("min", "-1", "1", (-1).into()),
            ("min", "1", "-1", (-1).into()),
            ("min", "1", "2", 1.into()),
            ("min", "2", "1", 1.into()),
            // angle
            // mindustry apparently uses a different algorithm that gives 29.999948501586914 instead of 30
            // but this is probably close enough
            ("angle", "0.8660254038", "0.5", 30.into()),
            // angleDiff
            ("angleDiff", "10", "10", 0.into()),
            ("angleDiff", "10", "20", 10.into()),
            ("angleDiff", "10", "-10", 20.into()),
            ("angleDiff", "10", "350", 20.into()),
            // len
            ("len", "3", "4", 5.into()),
            ("len", "1", "1", 2f32.sqrt().into()),
            // noise
            ("noise", "0", "0", 0.into()),
            // i'm not porting mindustry's noise algorithm. this is not the value you would get ingame
            ("noise", "0", "1", (-0.7139277281035279).into()),
            ("noise", "1", "0", (-0.3646124062135936).into()),
            // logn
            ("logn", "-1", "2", LValue::Null),
            ("logn", "0", "2", LValue::Null),
            ("logn", "0b1000", "2", 3.into()),
            ("logn", "0b1010", "2", 3.3219280948873626.into()),
            ("logn", "-1", "10", LValue::Null),
            ("logn", "0", "10", LValue::Null),
            ("logn", "100", "10", 2.into()),
            ("logn", "101", "10", 2.0043213737826426.into()),
        ]
        .into_iter()
        .chain(
            CONDITION_TESTS
                .iter()
                .filter(|&(op, ..)| *op != "always")
                .map(|&(op, x, y, want)| (op, x, y, want.into())),
        ) {
            let mut vm = single_processor_vm_with_globals(
                HYPER_PROCESSOR,
                &format!(
                    "
                    op {op} got {x} {y}
                    stop
                    "
                ),
                &globals,
            );

            run(&mut vm, 1, true);

            let state = take_processor(&mut vm, 0).state;
            assert_eq!(state.variables["got"].get(&state), want, "{op} {x} {y}");
        }
    }

    #[test]
    pub fn test_lookup() {
        let mut vm = single_processor_vm(
            HYPER_PROCESSOR,
            "
            set blocks @blockCount
            set items @itemCount
            set liquids @liquidCount
            set units @unitCount

            lookup block block1 -1
            lookup block block2 0
            lookup block block3 259
            lookup block block4 260

            lookup item item1 -1
            lookup item item2 0
            lookup item item3 19
            lookup item item4 20

            lookup liquid liquid1 -1
            lookup liquid liquid2 0
            lookup liquid liquid3 10
            lookup liquid liquid4 11

            lookup unit unit1 -1
            lookup unit unit2 0
            lookup unit unit3 55
            lookup unit unit4 56

            lookup team team1 -1
            lookup team team2 0
            lookup team team3 255
            lookup team team4 256

            stop
            ",
        );

        run(&mut vm, 1, true);

        let state = take_processor(&mut vm, 0).state;

        assert_eq!(state.variables["blocks"].get(&state), LValue::Number(260.));
        assert_eq!(state.variables["items"].get(&state), LValue::Number(20.));
        assert_eq!(state.variables["liquids"].get(&state), LValue::Number(11.));
        assert_eq!(state.variables["units"].get(&state), LValue::Number(56.));

        // blocks

        assert_eq!(state.variables["block1"].get(&state), LValue::Null);
        let block2 = state.variables["block2"].get(&state);
        assert!(
            matches!(block2, LValue::Content(Content::Block(b)) if b.name == "graphite-press"),
            "{block2:?}"
        );
        let block3 = state.variables["block3"].get(&state);
        assert!(
            matches!(block3, LValue::Content(Content::Block(b)) if b.name == "tile-logic-display"),
            "{block3:?}"
        );
        assert_eq!(state.variables["block4"].get(&state), LValue::Null);

        // items

        assert_eq!(state.variables["item1"].get(&state), LValue::Null);
        let item2 = state.variables["item2"].get(&state);
        assert!(
            matches!(item2, LValue::Content(Content::Item(b)) if b.name == "copper"),
            "{item2:?}"
        );
        let item3 = state.variables["item3"].get(&state);
        assert!(
            matches!(item3, LValue::Content(Content::Item(b)) if b.name == "carbide"),
            "{item3:?}"
        );
        assert_eq!(state.variables["item4"].get(&state), LValue::Null);

        // liquids

        assert_eq!(state.variables["liquid1"].get(&state), LValue::Null);
        let liquid2 = state.variables["liquid2"].get(&state);
        assert!(
            matches!(liquid2, LValue::Content(Content::Liquid(b)) if b.name == "water"),
            "{liquid2:?}"
        );
        let liquid3 = state.variables["liquid3"].get(&state);
        assert!(
            matches!(liquid3, LValue::Content(Content::Liquid(b)) if b.name == "arkycite"),
            "{liquid3:?}"
        );
        assert_eq!(state.variables["liquid4"].get(&state), LValue::Null);

        // units

        assert_eq!(state.variables["unit1"].get(&state), LValue::Null);
        let unit2 = state.variables["unit2"].get(&state);
        assert!(
            matches!(unit2, LValue::Content(Content::Unit(b)) if b.name == "dagger"),
            "{unit2:?}"
        );
        let unit3 = state.variables["unit3"].get(&state);
        assert!(
            matches!(unit3, LValue::Content(Content::Unit(b)) if b.name == "emanate"),
            "{unit3:?}"
        );
        assert_eq!(state.variables["unit4"].get(&state), LValue::Null);

        // teams

        assert_eq!(state.variables["team1"].get(&state), LValue::Null);
        assert_eq!(
            state.variables["team2"].get(&state),
            LValue::Team(Team::Derelict)
        );
        assert_eq!(
            state.variables["team3"].get(&state),
            LValue::Team(Team::Unknown(255))
        );
        assert_eq!(state.variables["team4"].get(&state), LValue::Null);
    }
}
