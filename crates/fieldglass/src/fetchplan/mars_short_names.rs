//! MARS short names of ECMWF's open-data parameters — generated, do not edit.
//!
//! Do not edit: regenerate with `python3 tools/gen_mars_short_names.py`.
//! Source: eccodes 2.34.1 and 2.45.0 and 2.48.0, their `shortName` concept for an
//! ECMWF GRIB2 message (`definitions/grib2/localConcepts/ecmf/` and
//! `definitions/grib2/`; Apache-2.0, factual data from ECMWF's parameter
//! database).
//!
//! 108 names, 192 blocks, and 280 blocks of other names
//! that can outrank them. Each block keeps the conditions the message identity
//! carries and counts every condition eccodes ranks it by; see the
//! generator's module docs for which and why.
//!
//! Also 5 blocks of eccodes' `marsLevtype` concept that
//! make a message `sfc` on a surface ECMWF lists levels of, and
//! 9 blocks of other levtypes on those surfaces (#1323).

/// An eccodes key this table keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// `discipline`.
    Discipline,
    /// `parameterCategory`.
    Category,
    /// `parameterNumber`.
    Number,
    /// `productDefinitionTemplateNumber`.
    ProductTemplate,
    /// `typeOfFirstFixedSurface`.
    FirstSurfaceType,
    /// `scaleFactorOfFirstFixedSurface`.
    FirstScaleFactor,
    /// `scaledValueOfFirstFixedSurface`.
    FirstScaledValue,
    /// `typeOfSecondFixedSurface`.
    SecondSurfaceType,
    /// `scaleFactorOfSecondFixedSurface`.
    SecondScaleFactor,
    /// `scaledValueOfSecondFixedSurface`.
    SecondScaledValue,
    /// `typeOfStatisticalProcessing`.
    StatisticalProcess,
    /// `indicatorOfUnitForTimeRange`.
    TimeRangeUnit,
    /// `lengthOfTimeRange`.
    TimeRangeLength,
    /// `probabilityType`.
    ProbabilityType,
    /// `scaleFactorOfLowerLimit`.
    LowerLimitScaleFactor,
    /// `scaledValueOfLowerLimit`.
    LowerLimitScaledValue,
}

/// One way a MARS short name is encoded: an eccodes concept block for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Block {
    /// The conditions this table compares, each a key and the value the block
    /// requires, `None` for `missing()`.
    pub read: &'static [(Key, Option<i32>)],
    /// How many conditions the eccodes block states, read here or not: what
    /// eccodes ranks matching blocks by.
    pub conditions: u8,
    /// The pools that hold the block, a bit each (see [`POOLS`]).
    pub pools: u8,
}

/// How many pools there are: the sets of concept files eccodes ranks blocks
/// within.
///
/// * bit 0: eccodes 2.34.1 `shortName.def`
/// * bit 1: eccodes 2.34.1 `shortName.legacy.def`
/// * bit 2: eccodes 2.45.0 `shortName.def`
/// * bit 3: eccodes 2.45.0 `shortName.lte33.def`
/// * bit 4: eccodes 2.45.0 `shortName.legacy.def`
/// * bit 5: eccodes 2.48.0 `shortName.def`
/// * bit 6: eccodes 2.48.0 `shortName.lte33.def`
/// * bit 7: eccodes 2.48.0 `shortName.legacy.def`
pub(crate) const POOLS: u32 = 8;

/// Every name the table carries, sorted, with its blocks in eccodes' lookup
/// order.
pub(crate) static NAMES: &[(&str, &[Block])] = &[
    (
        "100si",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(1)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(100)),
                    (Key::FirstScaleFactor, Some(0)),
                ],
                conditions: 6,
                pools: 0b01001101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(228)),
                    (Key::Number, Some(249)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "100u",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(2)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(100)),
                    (Key::FirstScaleFactor, Some(0)),
                ],
                conditions: 6,
                pools: 0b01001101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(228)),
                    (Key::Number, Some(246)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "100v",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(3)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(100)),
                    (Key::FirstScaleFactor, Some(0)),
                ],
                conditions: 6,
                pools: 0b01001101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(228)),
                    (Key::Number, Some(247)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "10fg",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(22)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(10)),
                (Key::FirstScaleFactor, Some(0)),
                (Key::StatisticalProcess, Some(2)),
            ],
            conditions: 7,
            pools: 0b01001001,
        }],
    ),
    (
        "10fgg10",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(22)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::StatisticalProcess, Some(2)),
                    (Key::LowerLimitScaledValue, Some(10)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 11,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(22)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::StatisticalProcess, Some(2)),
                    (Key::LowerLimitScaledValue, Some(10)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "10fgg15",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(22)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::StatisticalProcess, Some(2)),
                    (Key::LowerLimitScaledValue, Some(15)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 11,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(22)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::StatisticalProcess, Some(2)),
                    (Key::LowerLimitScaledValue, Some(15)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "10fgg25",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(22)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::StatisticalProcess, Some(2)),
                    (Key::LowerLimitScaledValue, Some(25)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 11,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(22)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::StatisticalProcess, Some(2)),
                    (Key::LowerLimitScaledValue, Some(25)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "10si",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(1)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(10)),
                (Key::FirstScaleFactor, Some(0)),
            ],
            conditions: 6,
            pools: 0b01101101,
        }],
    ),
    (
        "10spg10",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(68)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(133)),
                    (Key::Number, Some(21)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(1)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::LowerLimitScaledValue, Some(10)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "10spg15",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(69)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(133)),
                    (Key::Number, Some(22)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(1)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(10)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::LowerLimitScaledValue, Some(15)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "10u",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(2)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(10)),
                (Key::FirstScaleFactor, Some(0)),
            ],
            conditions: 6,
            pools: 0b01101101,
        }],
    ),
    (
        "10v",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(3)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(10)),
                (Key::FirstScaleFactor, Some(0)),
            ],
            conditions: 6,
            pools: 0b01101101,
        }],
    ),
    (
        "2d",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(6)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(2)),
                (Key::FirstScaleFactor, Some(0)),
            ],
            conditions: 6,
            pools: 0b01101101,
        }],
    ),
    (
        "2t",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(2)),
                (Key::FirstScaleFactor, Some(0)),
            ],
            conditions: 6,
            pools: 0b01101101,
        }],
    ),
    (
        "2tl273",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(73)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::FirstSurfaceType, Some(103)),
                    (Key::FirstScaledValue, Some(2)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::LowerLimitScaledValue, Some(27315)),
                    (Key::LowerLimitScaleFactor, Some(2)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 9,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "asn",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(19)),
                    (Key::Number, Some(192)),
                ],
                conditions: 4,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(19)),
                    (Key::Number, Some(19)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(32)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "cdww",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(16)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(140)),
                    (Key::Number, Some(233)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "cp",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(195)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 6,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(37)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
        ],
    ),
    (
        "d",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(13)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "ewss",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(62)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(180)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "fscov",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(1)),
                (Key::Number, Some(121)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "gh",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(3)),
                (Key::Number, Some(5)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "h1012",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 8,
            pools: 0b01101101,
        }],
    ),
    (
        "h1214",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 8,
            pools: 0b01101101,
        }],
    ),
    (
        "h1417",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 8,
            pools: 0b01101101,
        }],
    ),
    (
        "h1721",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 8,
            pools: 0b01101101,
        }],
    ),
    (
        "h2125",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 8,
            pools: 0b01101101,
        }],
    ),
    (
        "h2530",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 8,
            pools: 0b01101101,
        }],
    ),
    (
        "hcc",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(195)),
                ],
                conditions: 4,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(5)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(188)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "lcc",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(193)),
                ],
                conditions: 4,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(3)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(186)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "lsm",
        &[Block {
            read: &[
                (Key::Discipline, Some(2)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(1)),
            ],
            conditions: 4,
            pools: 0b01101101,
        }],
    ),
    (
        "mcc",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(194)),
                ],
                conditions: 4,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(4)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(187)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "mn2t3",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(2)),
                (Key::FirstScaleFactor, Some(0)),
                (Key::TimeRangeUnit, Some(1)),
                (Key::StatisticalProcess, Some(3)),
                (Key::TimeRangeLength, Some(3)),
            ],
            conditions: 9,
            pools: 0b01001001,
        }],
    ),
    (
        "mn2t6",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(2)),
                (Key::FirstScaleFactor, Some(0)),
                (Key::TimeRangeUnit, Some(1)),
                (Key::StatisticalProcess, Some(3)),
                (Key::TimeRangeLength, Some(6)),
            ],
            conditions: 9,
            pools: 0b01001001,
        }],
    ),
    (
        "mp2",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(28)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(140)),
                    (Key::Number, Some(221)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "msl",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(3)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(101)),
            ],
            conditions: 4,
            pools: 0b01101101,
        }],
    ),
    (
        "mucape",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(7)),
                (Key::Number, Some(6)),
                (Key::FirstSurfaceType, Some(17)),
                (Key::FirstScaledValue, None),
                (Key::FirstScaleFactor, None),
            ],
            conditions: 6,
            pools: 0b01101101,
        }],
    ),
    (
        "mwd",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(14)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "mwp",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(15)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "mwpg10",
        &[Block {
            read: &[
                (Key::Discipline, Some(192)),
                (Key::Category, Some(131)),
                (Key::Number, Some(79)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "mwpg12",
        &[Block {
            read: &[
                (Key::Discipline, Some(192)),
                (Key::Category, Some(131)),
                (Key::Number, Some(80)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "mwpg15",
        &[Block {
            read: &[
                (Key::Discipline, Some(192)),
                (Key::Category, Some(131)),
                (Key::Number, Some(81)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "mwpg8",
        &[Block {
            read: &[
                (Key::Discipline, Some(192)),
                (Key::Category, Some(131)),
                (Key::Number, Some(78)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "mx2t3",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(2)),
                (Key::FirstScaleFactor, Some(0)),
                (Key::TimeRangeUnit, Some(1)),
                (Key::StatisticalProcess, Some(2)),
                (Key::TimeRangeLength, Some(3)),
            ],
            conditions: 9,
            pools: 0b01001001,
        }],
    ),
    (
        "mx2t6",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(103)),
                (Key::FirstScaledValue, Some(2)),
                (Key::FirstScaleFactor, Some(0)),
                (Key::TimeRangeUnit, Some(1)),
                (Key::StatisticalProcess, Some(2)),
                (Key::TimeRangeLength, Some(6)),
            ],
            conditions: 9,
            pools: 0b01001001,
        }],
    ),
    (
        "nsss",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(63)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(181)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "pp1d",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(34)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(140)),
                    (Key::Number, Some(231)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "ptsa_gt_1p5stdev",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(15)),
                    (Key::LowerLimitScaleFactor, Some(1)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(15)),
                    (Key::LowerLimitScaleFactor, Some(1)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ptsa_gt_1stdev",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(1)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(1)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ptsa_gt_2stdev",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ptsa_lt_1p5stdev",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(-15)),
                    (Key::LowerLimitScaleFactor, Some(1)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(-15)),
                    (Key::LowerLimitScaleFactor, Some(1)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 7,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ptsa_lt_1stdev",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(-1)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(-1)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 7,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ptsa_lt_2stdev",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(-2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(0)),
                    (Key::StatisticalProcess, Some(10)),
                    (Key::LowerLimitScaledValue, Some(-2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 7,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ptype",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(1)),
                (Key::Number, Some(19)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "q",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(1)),
                (Key::Number, Some(0)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "r",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(1)),
                (Key::Number, Some(1)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "ro",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(2)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(201)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(2)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(33)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(205)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "rowe",
        &[Block {
            read: &[
                (Key::Discipline, Some(2)),
                (Key::Category, Some(0)),
                (Key::Number, Some(42)),
                (Key::StatisticalProcess, Some(1)),
            ],
            conditions: 4,
            pools: 0b01101101,
        }],
    ),
    (
        "rsn",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(61)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(33)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "sd",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(254)),
                ],
                conditions: 4,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(60)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
        ],
    ),
    (
        "sdor",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(20)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(160)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "sf",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(198)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 6,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(53)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(144)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "sithick",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(1)),
                    (Key::FirstSurfaceType, Some(174)),
                    (Key::SecondSurfaceType, Some(176)),
                    (Key::FirstScaledValue, None),
                    (Key::FirstScaleFactor, None),
                    (Key::SecondScaledValue, None),
                    (Key::SecondScaleFactor, None),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(174)),
                    (Key::Number, Some(98)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(1)),
                ],
                conditions: 3,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "skt",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(17)),
                (Key::FirstSurfaceType, Some(1)),
            ],
            conditions: 4,
            pools: 0b01101101,
        }],
    ),
    (
        "slor",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(22)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(163)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "sot",
        &[Block {
            read: &[
                (Key::Discipline, Some(2)),
                (Key::Category, Some(3)),
                (Key::Number, Some(18)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "sp",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(3)),
                (Key::Number, Some(0)),
                (Key::FirstSurfaceType, Some(1)),
            ],
            conditions: 4,
            pools: 0b01101101,
        }],
    ),
    (
        "ssr",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(180)),
                    (Key::Number, Some(176)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(4)),
                    (Key::Number, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
        ],
    ),
    (
        "ssrd",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(4)),
                    (Key::Number, Some(7)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(169)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "str",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(180)),
                    (Key::Number, Some(177)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(5)),
                    (Key::Number, Some(5)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
        ],
    ),
    (
        "strd",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(5)),
                    (Key::Number, Some(3)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(175)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "sve",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(14)),
                    (Key::FirstSurfaceType, Some(160)),
                    (Key::SecondSurfaceType, Some(255)),
                    (Key::FirstScaledValue, Some(0)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::SecondScaledValue, None),
                    (Key::SecondScaleFactor, None),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(14)),
                ],
                conditions: 3,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "svn",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(15)),
                    (Key::FirstSurfaceType, Some(160)),
                    (Key::SecondSurfaceType, Some(255)),
                    (Key::FirstScaledValue, Some(0)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::SecondScaledValue, None),
                    (Key::SecondScaleFactor, None),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(15)),
                ],
                conditions: 3,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "swh",
        &[Block {
            read: &[
                (Key::Discipline, Some(10)),
                (Key::Category, Some(0)),
                (Key::Number, Some(3)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "swhg2",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::ProductTemplate, Some(5)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b00001100,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::LowerLimitScaledValue, Some(2)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 6,
                pools: 0b01100000,
            },
        ],
    ),
    (
        "swhg4",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::ProductTemplate, Some(5)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(4)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(4)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b00001100,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::LowerLimitScaledValue, Some(4)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 6,
                pools: 0b01100000,
            },
        ],
    ),
    (
        "swhg6",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::ProductTemplate, Some(5)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(6)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(6)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b00001100,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::LowerLimitScaledValue, Some(6)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 6,
                pools: 0b01100000,
            },
        ],
    ),
    (
        "swhg8",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::ProductTemplate, Some(5)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(8)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::FirstSurfaceType, Some(101)),
                    (Key::LowerLimitScaledValue, Some(8)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 7,
                pools: 0b00001100,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(0)),
                    (Key::Number, Some(3)),
                    (Key::LowerLimitScaledValue, Some(8)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 6,
                pools: 0b01100000,
            },
        ],
    ),
    (
        "t",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(0)),
                (Key::Number, Some(0)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "tcc",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(192)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(8)),
                ],
                conditions: 6,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(6)),
                    (Key::Number, Some(1)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(8)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(164)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "tciw",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(70)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(8)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(79)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "tclw",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(69)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(8)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(78)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "tcrw",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(45)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(8)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(228)),
                    (Key::Number, Some(89)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "tcw",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(1)),
                (Key::Number, Some(51)),
                (Key::FirstSurfaceType, Some(1)),
                (Key::SecondSurfaceType, Some(8)),
            ],
            conditions: 5,
            pools: 0b01101101,
        }],
    ),
    (
        "tcwv",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(64)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(8)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(128)),
                    (Key::Number, Some(137)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "tp",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(193)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::SecondSurfaceType, Some(255)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 7,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
        ],
    ),
    (
        "tpg1",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(1)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(60)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(1)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpg10",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(10)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(10)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpg100",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(100)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(85)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(100)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpg20",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(20)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(20)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpg25",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(25)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(25)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpg5",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(5)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(61)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(5)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpg50",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::ProductTemplate, Some(9)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(50)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(50)),
                    (Key::LowerLimitScaleFactor, Some(0)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 8,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tpl01",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(64)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::FirstScaledValue, None),
                    (Key::FirstScaleFactor, None),
                    (Key::StatisticalProcess, Some(1)),
                    (Key::LowerLimitScaledValue, Some(1)),
                    (Key::LowerLimitScaleFactor, Some(1)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tprate",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(172)),
                    (Key::Number, Some(228)),
                ],
                conditions: 3,
                pools: 0b10010011,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                ],
                conditions: 4,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(193)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::StatisticalProcess, Some(0)),
                ],
                conditions: 6,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tprg3",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(66)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::FirstScaledValue, None),
                    (Key::FirstScaleFactor, None),
                    (Key::StatisticalProcess, Some(0)),
                    (Key::LowerLimitScaledValue, Some(347222)),
                    (Key::LowerLimitScaleFactor, Some(10)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tprg5",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(67)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::FirstScaledValue, None),
                    (Key::FirstScaleFactor, None),
                    (Key::StatisticalProcess, Some(0)),
                    (Key::LowerLimitScaledValue, Some(578704)),
                    (Key::LowerLimitScaleFactor, Some(10)),
                    (Key::ProbabilityType, Some(3)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "tprl1",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(131)),
                    (Key::Number, Some(65)),
                ],
                conditions: 3,
                pools: 0b10010001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(1)),
                    (Key::Number, Some(52)),
                    (Key::FirstSurfaceType, Some(1)),
                    (Key::FirstScaledValue, None),
                    (Key::FirstScaleFactor, None),
                    (Key::StatisticalProcess, Some(0)),
                    (Key::LowerLimitScaledValue, Some(115741)),
                    (Key::LowerLimitScaleFactor, Some(10)),
                    (Key::ProbabilityType, Some(0)),
                ],
                conditions: 10,
                pools: 0b01101100,
            },
        ],
    ),
    (
        "ttr",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(170)),
                    (Key::Number, Some(179)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(180)),
                    (Key::Number, Some(179)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(5)),
                    (Key::Number, Some(5)),
                    (Key::FirstSurfaceType, Some(8)),
                    (Key::StatisticalProcess, Some(1)),
                ],
                conditions: 5,
                pools: 0b01101101,
            },
        ],
    ),
    (
        "u",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(2)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "v",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(3)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "vo",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(0)),
                    (Key::Category, Some(2)),
                    (Key::Number, Some(12)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(4)),
                    (Key::Number, Some(26)),
                    (Key::FirstSurfaceType, Some(168)),
                    (Key::SecondSurfaceType, Some(168)),
                ],
                conditions: 5,
                pools: 0b00000001,
            },
        ],
    ),
    (
        "vsw",
        &[Block {
            read: &[
                (Key::Discipline, Some(2)),
                (Key::Category, Some(0)),
                (Key::Number, Some(25)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "w",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(8)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "wmb",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(4)),
                    (Key::Number, Some(7)),
                ],
                conditions: 3,
                pools: 0b01101101,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(140)),
                    (Key::Number, Some(219)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
        ],
    ),
    (
        "ws",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(2)),
                (Key::Number, Some(1)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "z",
        &[Block {
            read: &[
                (Key::Discipline, Some(0)),
                (Key::Category, Some(3)),
                (Key::Number, Some(4)),
            ],
            conditions: 3,
            pools: 0b01101101,
        }],
    ),
    (
        "zos",
        &[
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(1)),
                    (Key::FirstSurfaceType, Some(160)),
                    (Key::SecondSurfaceType, Some(255)),
                    (Key::FirstScaledValue, Some(0)),
                    (Key::FirstScaleFactor, Some(0)),
                    (Key::SecondScaledValue, None),
                    (Key::SecondScaleFactor, None),
                ],
                conditions: 9,
                pools: 0b00000001,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(192)),
                    (Key::Category, Some(151)),
                    (Key::Number, Some(145)),
                ],
                conditions: 3,
                pools: 0b10010010,
            },
            Block {
                read: &[
                    (Key::Discipline, Some(10)),
                    (Key::Category, Some(3)),
                    (Key::Number, Some(1)),
                ],
                conditions: 3,
                pools: 0b01101100,
            },
        ],
    ),
];

/// Blocks of names not in [`NAMES`] that share parameter codes with a block of
/// one there, in the same pool, and could outrank it: compared only as
/// rivals, never as the name a record gives.
pub(crate) static RIVALS: &[Block] = &[
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(2)),
            (Key::TimeRangeLength, Some(24)),
        ],
        conditions: 9,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(3)),
            (Key::TimeRangeLength, Some(24)),
        ],
        conditions: 9,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(40)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 9,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(60)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 9,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(80)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 9,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(150)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 9,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(200)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 9,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(3)),
            (Key::LowerLimitScaleFactor, Some(-2)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 9,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(22)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(2)),
            (Key::TimeRangeLength, Some(3)),
        ],
        conditions: 9,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(2)),
            (Key::TimeRangeLength, Some(6)),
        ],
        conditions: 8,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(7)),
            (Key::SecondSurfaceType, Some(255)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
        ],
        conditions: 6,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(200)),
            (Key::FirstScaleFactor, Some(0)),
        ],
        conditions: 6,
        pools: 0b01001101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(200)),
            (Key::FirstScaleFactor, Some(0)),
        ],
        conditions: 6,
        pools: 0b01001101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(200)),
            (Key::FirstScaleFactor, Some(0)),
        ],
        conditions: 6,
        pools: 0b01001101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(185)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(186)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(187)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(60)),
            (Key::FirstSurfaceType, Some(185)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(60)),
            (Key::FirstSurfaceType, Some(187)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(187)),
            (Key::SecondSurfaceType, Some(185)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(187)),
            (Key::SecondSurfaceType, Some(185)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(60)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(17)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 5,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(100)),
            (Key::TimeRangeLength, Some(1)),
        ],
        conditions: 6,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(100)),
            (Key::TimeRangeLength, Some(3)),
        ],
        conditions: 6,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(101)),
            (Key::TimeRangeLength, Some(1)),
        ],
        conditions: 6,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(101)),
            (Key::TimeRangeLength, Some(3)),
        ],
        conditions: 6,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(100)),
            (Key::TimeRangeLength, Some(6)),
        ],
        conditions: 6,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::TimeRangeUnit, Some(1)),
            (Key::StatisticalProcess, Some(101)),
            (Key::TimeRangeLength, Some(6)),
        ],
        conditions: 6,
        pools: 0b01001001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(4)),
            (Key::Number, Some(9)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
        ],
        conditions: 9,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(102)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
        ],
        conditions: 9,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
        ],
        conditions: 9,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(174)),
            (Key::SecondSurfaceType, Some(176)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(15)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(14)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(4)),
            (Key::Number, Some(26)),
            (Key::FirstSurfaceType, Some(168)),
            (Key::SecondSurfaceType, Some(168)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(4)),
            (Key::Number, Some(9)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(102)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(4)),
            (Key::Number, Some(9)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(1)),
        ],
        conditions: 10,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(1)),
        ],
        conditions: 10,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(22)),
            (Key::ProductTemplate, Some(9)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
            (Key::LowerLimitScaledValue, Some(20)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 11,
        pools: 0b00000001,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(5)),
            (Key::FirstSurfaceType, Some(1)),
        ],
        conditions: 4,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101101,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(40)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(60)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(80)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(150)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(200)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(1)),
            (Key::LowerLimitScaledValue, Some(3)),
            (Key::LowerLimitScaleFactor, Some(-2)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(28)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(34)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(16)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(28)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(34)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(16)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(28)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(34)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(16)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(28)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(34)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(0)),
            (Key::Number, Some(16)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(2)),
            (Key::SecondScaleFactor, Some(1)),
        ],
        conditions: 9,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(1)),
            (Key::SecondScaleFactor, Some(0)),
        ],
        conditions: 9,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(19)),
            (Key::FirstScaledValue, Some(50)),
            (Key::FirstScaleFactor, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(167)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(61)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(69)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(70)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(121)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(5)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(45)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(2)),
            (Key::SecondScaleFactor, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(1)),
            (Key::SecondScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(2)),
            (Key::SecondScaleFactor, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(1)),
            (Key::SecondScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 10,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(51)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(64)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(12)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(101)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(13)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(19)),
            (Key::Number, Some(19)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(7)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(5000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(10000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(167)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(60)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(17)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(61)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(69)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(70)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(121)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(5)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(45)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(51)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(64)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(12)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(101)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(13)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(19)),
            (Key::Number, Some(19)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(7)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(5000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(10000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(167)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(60)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(17)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(61)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(69)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(70)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(121)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(5)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(45)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(51)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(64)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(12)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(101)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(13)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(19)),
            (Key::Number, Some(19)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(7)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(5000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(10000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(167)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(60)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(17)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(61)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(69)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(70)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 8,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(121)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(3)),
            (Key::Number, Some(18)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(5)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(45)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(4)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(51)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(64)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(12)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(101)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 5,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(13)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(2)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(3)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(0)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(2)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(19)),
            (Key::Number, Some(19)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(6)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(8)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(7)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(5000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(7)),
            (Key::Number, Some(6)),
            (Key::FirstSurfaceType, Some(18)),
            (Key::FirstScaledValue, Some(10000)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(3)),
            (Key::Number, Some(0)),
            (Key::FirstSurfaceType, Some(17)),
            (Key::FirstScaledValue, None),
            (Key::FirstScaleFactor, None),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(1)),
            (Key::SecondSurfaceType, Some(167)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 6,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(2)),
            (Key::SecondScaleFactor, Some(1)),
        ],
        conditions: 9,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(2)),
            (Key::Category, Some(0)),
            (Key::Number, Some(25)),
            (Key::FirstSurfaceType, Some(106)),
            (Key::SecondSurfaceType, Some(106)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(1)),
            (Key::SecondScaleFactor, Some(0)),
        ],
        conditions: 9,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::StatisticalProcess, Some(100)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(19)),
            (Key::StatisticalProcess, Some(101)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 4,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(4)),
            (Key::Number, Some(9)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(102)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(2)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(4)),
            (Key::Number, Some(9)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(102)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(3)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(1)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(15)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(10)),
            (Key::Category, Some(3)),
            (Key::Number, Some(14)),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 4,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(4)),
            (Key::Number, Some(9)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(102)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(1)),
            (Key::Number, Some(52)),
            (Key::FirstSurfaceType, Some(160)),
            (Key::SecondSurfaceType, Some(255)),
            (Key::FirstScaledValue, Some(0)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::SecondScaledValue, None),
            (Key::SecondScaleFactor, None),
            (Key::StatisticalProcess, Some(6)),
        ],
        conditions: 10,
        pools: 0b00100100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(1)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(0)),
        ],
        conditions: 7,
        pools: 0b01101100,
    },
    Block {
        read: &[
            (Key::Discipline, Some(0)),
            (Key::Category, Some(2)),
            (Key::Number, Some(22)),
            (Key::FirstSurfaceType, Some(103)),
            (Key::FirstScaledValue, Some(10)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::StatisticalProcess, Some(2)),
            (Key::LowerLimitScaledValue, Some(20)),
            (Key::LowerLimitScaleFactor, Some(0)),
            (Key::ProbabilityType, Some(3)),
        ],
        conditions: 10,
        pools: 0b01001000,
    },
];

/// How many levtype pools there are: the sets of `marsLevtype` concept files
/// eccodes ranks blocks within. [`SFC_ON_LEVEL_LISTS`] and
/// [`LEVTYPES_ON_LEVEL_LISTS`] mark these in [`Block::pools`], not [`POOLS`].
///
/// * bit 0: eccodes 2.34.1 `marsLevtypeConcept.def`
/// * bit 1: eccodes 2.45.0 `marsLevtypeConcept.def`
/// * bit 2: eccodes 2.48.0 `marsLevtypeConcept.def`
/// * bit 3: eccodes 2.48.0 `marsLevtypeConcept.lte33.def`
pub(crate) const LEVTYPE_POOLS: u32 = 4;

/// The `marsLevtype` blocks that make a message `sfc` although its first
/// fixed surface is one ECMWF lists levels of (100, 105, 107, 109, 151).
pub(crate) static SFC_ON_LEVEL_LISTS: &[Block] = &[
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(100)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::FirstScaledValue, Some(80000)),
            (Key::SecondSurfaceType, Some(100)),
            (Key::SecondScaleFactor, Some(0)),
            (Key::SecondScaledValue, Some(45000)),
        ],
        conditions: 6,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(100)),
            (Key::FirstScaleFactor, Some(0)),
            (Key::FirstScaledValue, Some(45000)),
            (Key::SecondSurfaceType, Some(8)),
        ],
        conditions: 4,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(107)),
            (Key::FirstScaleFactor, Some(2)),
            (Key::FirstScaledValue, Some(27315)),
        ],
        conditions: 3,
        pools: 0b1110,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(107)),
            (Key::FirstScaleFactor, Some(2)),
            (Key::FirstScaledValue, Some(27415)),
        ],
        conditions: 3,
        pools: 0b1110,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(107)),
            (Key::FirstScaleFactor, Some(2)),
            (Key::FirstScaledValue, Some(27465)),
        ],
        conditions: 3,
        pools: 0b1110,
    },
];

/// The `marsLevtype` blocks of every other levtype whose first fixed surface
/// is one of those: compared only as rivals of [`SFC_ON_LEVEL_LISTS`].
pub(crate) static LEVTYPES_ON_LEVEL_LISTS: &[Block] = &[
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(100)),
            (Key::SecondSurfaceType, Some(255)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(100)),
            (Key::SecondSurfaceType, Some(100)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(105)),
            (Key::SecondSurfaceType, Some(255)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(105)),
            (Key::SecondSurfaceType, Some(105)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(107)),
            (Key::SecondSurfaceType, Some(255)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(107)),
            (Key::SecondSurfaceType, Some(107)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(109)),
            (Key::SecondSurfaceType, Some(255)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(151)),
            (Key::SecondSurfaceType, Some(255)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
    Block {
        read: &[
            (Key::FirstSurfaceType, Some(151)),
            (Key::SecondSurfaceType, Some(151)),
        ],
        conditions: 2,
        pools: 0b1111,
    },
];

/// The blocks that define `name`, or `None` for a name this table does not
/// carry.
pub(crate) fn blocks(name: &str) -> Option<&'static [Block]> {
    NAMES
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| NAMES[i].1)
}
