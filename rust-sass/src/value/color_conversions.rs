// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/conversions.dart (matrices, d50);
//   conversion logic: lib/src/value/color/space.dart (ColorSpace.convertLinear),
//   Lab f-helpers: lib/src/value/color/space/xyz_d50.dart, space/lab.dart
// go-source: go/value/color_conversions.go

use crate::value::color_space_lms::lms_convert_internal;
use crate::value::color_space_srgb::srgb_convert_internal;
use crate::value::color_space_xyz_d50::xyz_d50_convert_internal;
use std::sync::LazyLock;

use crate::common::exception::SassResult;
use crate::math;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_utils::{LAB_EPSILON, LAB_KAPPA};

// Conversion matrices and the shared linear-transform hub.
//
// Matches Dart: the matrices live in `conversions.dart`, while the logic here
// is `ColorSpace.convertLinear` (`space.dart`) with the Lab f-helpers from
// the xyz-d50/lab space files. The per-space `convert` overrides live in the
// individual `color_space_*.rs` modules; `convert_color` below is the
// Rust-side dispatch hub that replaces Dart's virtual `ColorSpace.convert`.

// The D50 white point, from the Color Level 4 conversion spec.
pub(crate) static D50: LazyLock<[f64; 3]> = LazyLock::new(|| {
    let x = 0.3457;
    let y = 0.3585;
    [x / y, 1.0, (1.0 - x - y) / y]
});

// ==== All 52 color conversion matrices ====
//
// Matrix values from the Color Level 4 conversion code; the RGB-to-RGB pairs
// below it were precomputed offline (see Dart's pointer to the gist).

// Converts LMS channels to OKLab.
//
// Cannot be multiplied directly with the xyz-d65-to-LMS matrix; converting
// between XYZ and OKLab needs the spec's nonlinear steps around it.
pub(crate) const LMS_TO_OKLAB: [f64; 9] = [
    0.210_454_268_309_314,
    0.793_617_774_702_305_4,
    -0.00407204301161930,
    1.977_998_532_431_168_4,
    -2.428_592_242_048_58,
    0.450_593_709_617_411,
    0.025_904_042_465_547_8,
    0.782_771_712_457_529_6,
    -0.808_675_754_923_077_4,
];
// Converts OKLab channels back to LMS.
//
// Like its forward pair, this cannot be composed directly with the
// LMS-to-XYZ matrix; the XYZ↔OKLab path needs the spec's nonlinear steps.
pub(crate) const OKLAB_TO_LMS: [f64; 9] = [
    1.000_000_000_000_000_2,
    0.396_337_777_376_174_9,
    0.215_803_757_309_913_6,
    0.999_999_999_999_999_8,
    -0.10556134581565854,
    -0.06385417282581334,
    0.999_999_999_999_999_9,
    -0.089_484_177_529_811_8,
    -1.291_485_548_019_409_4,
];
pub(crate) const LINEAR_SRGB_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    0.822_461_968_714_362_3,
    0.17753803128563775,
    0.00000000000000000,
    0.03319419885096161,
    0.966_805_801_149_038_4,
    0.00000000000000000,
    0.01708263072112003,
    0.07239744066396346,
    0.910_519_928_614_916_5,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_LINEAR_SRGB: [f64; 9] = [
    1.224_940_176_280_559_8,
    -0.22494017628055996,
    0.00000000000000000,
    -0.04205695470968816,
    1.042_056_954_709_688,
    0.00000000000000000,
    -0.01963755459033443,
    -0.07863604555063188,
    1.098_273_600_140_966_3,
];
pub(crate) const LINEAR_SRGB_TO_LINEAR_A98_RGB: [f64; 9] = [
    0.715_125_606_855_624_7,
    0.28487439314437535,
    0.00000000000000000,
    0.00000000000000000,
    1.00000000000000000,
    0.00000000000000000,
    0.00000000000000000,
    0.04116194845011846,
    0.958_838_051_549_881_6,
];
pub(crate) const LINEAR_A98_RGB_TO_LINEAR_SRGB: [f64; 9] = [
    1.398_355_743_960_778_3,
    -0.398_355_743_960_778_3,
    0.00000000000000000,
    0.00000000000000000,
    1.00000000000000000,
    0.00000000000000000,
    0.00000000000000000,
    -0.04292898929447326,
    1.042_928_989_294_473_3,
];
pub(crate) const LINEAR_SRGB_TO_LINEAR_REC2020: [f64; 9] = [
    0.627_403_895_934_699,
    0.329_283_038_377_883_7,
    0.04331306568741722,
    0.06909728935823208,
    0.919_540_395_075_458_7,
    0.01136231556630917,
    0.01639143887515027,
    0.08801330787722575,
    0.895_595_253_247_624,
];
pub(crate) const LINEAR_REC2020_TO_LINEAR_SRGB: [f64; 9] = [
    1.660_491_002_108_434_5,
    -0.587_641_138_788_549_5,
    -0.07284986331988487,
    -0.12455047452159074,
    1.132_899_897_125_960_3,
    -0.00834942260436947,
    -0.018_150_763_354_905_3,
    -0.10057889800800737,
    1.118_729_661_362_912_7,
];
pub(crate) const LINEAR_SRGB_TO_XYZ_D65: [f64; 9] = [
    0.412_390_799_265_959_5,
    0.35758433938387796,
    0.180_480_788_401_834_3,
    0.21263900587151036,
    0.715_168_678_767_755_9,
    0.07219231536073371,
    0.01933081871559185,
    0.11919477979462598,
    0.950_532_152_249_660_6,
];
pub(crate) const XYZ_D65_TO_LINEAR_SRGB: [f64; 9] = [
    3.240_969_941_904_521_3,
    -1.537_383_177_570_093_5,
    -0.498_610_760_293_003_3,
    -0.969_243_636_280_879_8,
    1.875_967_501_507_720_6,
    0.04155505740717561,
    0.055_630_079_696_993_6,
    -0.20397695888897657,
    1.056_971_514_242_878_6,
];
pub(crate) const LINEAR_SRGB_TO_LMS: [f64; 9] = [
    0.412_221_469_470_763,
    0.536_332_537_261_734_8,
    0.051_445_993_267_502_2,
    0.211_903_495_817_825_2,
    0.680_699_550_645_234_2,
    0.107_396_953_536_940_5,
    0.08830245919005641,
    0.281_718_839_136_121_5,
    0.629_978_701_673_822_1,
];
pub(crate) const LMS_TO_LINEAR_SRGB: [f64; 9] = [
    4.076_741_636_075_958,
    -3.307_711_539_258_062,
    0.23096990318210417,
    -1.268_437_973_285_032,
    2.609_757_349_287_689,
    -0.341_319_376_002_657_1,
    -0.00419607613867551,
    -0.703_418_617_935_936_3,
    1.707_614_694_074_612,
];
pub(crate) const LINEAR_SRGB_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    0.529_276_977_622_611_6,
    0.33015450197849283,
    0.14056852039889556,
    0.09836585954044917,
    0.873_470_712_906_961_8,
    0.028_163_427_552_589,
    0.01687534092138684,
    0.11765941425612084,
    0.865_465_244_822_492_3,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_LINEAR_SRGB: [f64; 9] = [
    2.034_380_849_516_996,
    -0.727_635_789_934_134_2,
    -0.306_745_059_582_861_8,
    -0.22882573163305037,
    1.231_742_541_190_104_8,
    -0.00291680955705449,
    -0.00855882878391742,
    -0.153_266_702_138_037_2,
    1.161_825_530_921_954_7,
];
pub(crate) const LINEAR_SRGB_TO_XYZ_D50: [f64; 9] = [
    0.43606574687426936,
    0.385_151_509_590_159_6,
    0.14307841996513868,
    0.22249317711056518,
    0.716_887_013_094_482_4,
    0.06061980979495235,
    0.01392392146316939,
    0.09708132423141015,
    0.714_099_356_815_880_7,
];
pub(crate) const XYZ_D50_TO_LINEAR_SRGB: [f64; 9] = [
    3.134_135_852_900_117_8,
    -1.617_385_998_018_042,
    -0.49066221791109754,
    -0.978_795_476_555_777_7,
    1.916_254_377_395_988_4,
    0.03344287339036693,
    0.07195539255794733,
    -0.228_976_759_815_182,
    1.405_386_035_113_118_2,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_LINEAR_A98_RGB: [f64; 9] = [
    0.864_005_137_474_048_4,
    0.13599486252595164,
    0.00000000000000000,
    -0.04205695470968816,
    1.042_056_954_709_688,
    0.00000000000000000,
    -0.02056038078232985,
    -0.03250613804550798,
    1.053_066_518_827_837_9,
];
pub(crate) const LINEAR_A98_RGB_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    1.150_094_418_141_018_4,
    -0.15009441814101834,
    0.00000000000000000,
    0.04641729862941844,
    0.953_582_701_370_581_5,
    0.00000000000000000,
    0.02388759479083904,
    0.02650477632633013,
    0.949_607_628_882_830_8,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_LINEAR_REC2020: [f64; 9] = [
    0.753_833_034_361_721_8,
    0.198_597_369_052_616_3,
    0.04756959658566187,
    0.04574384896535833,
    0.941_777_219_811_693_5,
    0.01247893122294812,
    -0.00121034035451832,
    0.01760171730108989,
    0.983_608_623_053_428_4,
];
pub(crate) const LINEAR_REC2020_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    1.343_578_252_584_332,
    -0.282_179_670_526_135_7,
    -0.06139858205819628,
    -0.06529745278911953,
    1.075_787_915_848_574_6,
    -0.01049046305945495,
    0.00282178726170095,
    -0.01959849452449406,
    1.016_776_707_262_793_1,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_XYZ_D65: [f64; 9] = [
    0.48657094864821626,
    0.26566769316909294,
    0.198_217_285_234_362_5,
    0.22897456406974884,
    0.691_738_521_836_506_2,
    0.079_286_914_093_745,
    0.00000000000000000,
    0.04511338185890257,
    1.043_944_368_900_975_7,
];
pub(crate) const XYZ_D65_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    2.493_496_911_941_424_5,
    -0.931_383_617_919_123_6,
    -0.40271078445071684,
    -0.829_488_969_561_574_9,
    1.762_664_060_318_346_8,
    0.02362468584194359,
    0.03584583024378433,
    -0.076_172_389_268_041_7,
    0.956_884_524_007_687_3,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_LMS: [f64; 9] = [
    0.48137985274995443,
    0.46211837101131803,
    0.05650177623872756,
    0.22883194181124472,
    0.653_216_819_383_567_6,
    0.11795123880518774,
    0.08394575232299319,
    0.22416527097756642,
    0.691_888_976_699_440_4,
];
pub(crate) const LMS_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    3.127_768_971_361_873_7,
    -2.257_135_762_591_638_6,
    0.12936679122976494,
    -1.091_009_018_437_797_9,
    2.413_331_710_306_922_5,
    -0.32232269186912466,
    -0.02601080193857045,
    -0.508_041_331_704_167,
    1.534_052_133_642_737_3,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    0.631_686_919_340_358_9,
    0.21393038569465722,
    0.154_382_694_964_983_9,
    0.08320371426648458,
    0.885_865_136_763_024_3,
    0.03093114897049121,
    -0.00127273456473881,
    0.05075510433665735,
    0.950_517_630_228_081_4,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    1.632_575_608_706_917_9,
    -0.379_771_618_482_598_4,
    -0.252_803_990_224_319_5,
    -0.15370040233755072,
    1.166_702_547_242_501_4,
    -0.01300214490495082,
    0.01039319529676572,
    -0.062_807_312_649_594_4,
    1.052_414_117_352_828_7,
];
pub(crate) const LINEAR_DISPLAY_P3_TO_XYZ_D50: [f64; 9] = [
    0.515_146_442_968_116,
    0.292_009_982_063_857_7,
    0.15713925139759397,
    0.241_200_322_125_255_2,
    0.692_222_541_131_381_8,
    0.06657713674336294,
    -0.00105013914714014,
    0.041_878_270_189_074_6,
    0.784_276_471_468_525_7,
];
pub(crate) const XYZ_D50_TO_LINEAR_DISPLAY_P3: [f64; 9] = [
    2.403_934_121_855_497_3,
    -0.990_030_442_495_593_1,
    -0.39761363181465614,
    -0.842_270_016_145_468_8,
    1.798_958_016_106_708_2,
    0.01604562477090472,
    0.04819381686413303,
    -0.09738519815446048,
    1.273_671_369_332_127_3,
];
pub(crate) const LINEAR_A98_RGB_TO_LINEAR_REC2020: [f64; 9] = [
    0.877_333_841_663_656_8,
    0.07749370651571998,
    0.04517245182062317,
    0.09662259146620378,
    0.891_527_320_244_180_5,
    0.01185008828961569,
    0.02292106270284839,
    0.04303668501067932,
    0.934_042_252_286_472_3,
];
pub(crate) const LINEAR_REC2020_TO_LINEAR_A98_RGB: [f64; 9] = [
    1.151_978_394_715_916_3,
    -0.097_503_055_302_408_6,
    -0.05447533941350766,
    -0.12455047452159074,
    1.132_899_897_125_960_3,
    -0.00834942260436947,
    -0.022_530_382_781_055_9,
    -0.04980650742838876,
    1.072_336_890_209_444_6,
];
pub(crate) const LINEAR_A98_RGB_TO_XYZ_D65: [f64; 9] = [
    0.576_669_042_910_130_8,
    0.18555823790654627,
    0.18822864623499472,
    0.29734497525053616,
    0.627_363_566_255_466,
    0.07529145849399789,
    0.02703136138641237,
    0.07068885253582714,
    0.991_337_536_837_638_9,
];
pub(crate) const XYZ_D65_TO_LINEAR_A98_RGB: [f64; 9] = [
    2.041_587_903_810_746,
    -0.565_006_974_278_859_6,
    -0.344_731_350_778_329_5,
    -0.969_243_636_280_879_8,
    1.875_967_501_507_720_6,
    0.04155505740717561,
    0.01344428063203102,
    -0.11836239223101823,
    1.015_174_994_391_205_4,
];
pub(crate) const LINEAR_A98_RGB_TO_LMS: [f64; 9] = [
    0.576_432_259_618_394_1,
    0.36991322261987963,
    0.05365451776172635,
    0.29631647054222465,
    0.591_676_133_252_188_5,
    0.11200739620558686,
    0.123_478_251_014_277_6,
    0.21949869837199862,
    0.657_023_050_613_723_8,
];
pub(crate) const LMS_TO_LINEAR_A98_RGB: [f64; 9] = [
    2.554_036_838_611_556_6,
    -1.621_976_180_682_869_9,
    0.06793934207131327,
    -1.268_437_973_285_032,
    2.609_757_349_287_689,
    -0.341_319_376_002_657_1,
    -0.05623473593749381,
    -0.567_041_839_566_906_1,
    1.623_276_575_504_399_9,
];
pub(crate) const LINEAR_A98_RGB_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    0.740_117_501_804_779_2,
    0.11327951328898105,
    0.146_602_984_906_239_7,
    0.137_550_464_698_026_2,
    0.833_077_080_269_484,
    0.02937245503248977,
    0.02359772990871766,
    0.07378347703906656,
    0.902_618_793_052_215_8,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_LINEAR_A98_RGB: [f64; 9] = [
    1.389_651_248_151_52,
    -0.16945907691487766,
    -0.22019217123664242,
    -0.22882573163305037,
    1.231_742_541_190_104_8,
    -0.00291680955705449,
    -0.01762544368426068,
    -0.09625702306122665,
    1.113_882_466_745_487_4,
];
pub(crate) const LINEAR_A98_RGB_TO_XYZ_D50: [f64; 9] = [
    0.609_775_041_886_181_4,
    0.20530000261929401,
    0.14922063192409227,
    0.31112461220464155,
    0.625_653_230_834_685_6,
    0.06322215696067286,
    0.01947059555648168,
    0.06087908649415867,
    0.744_754_920_459_819_8,
];
pub(crate) const XYZ_D50_TO_LINEAR_A98_RGB: [f64; 9] = [
    1.962_467_036_376_880_6,
    -0.610_742_340_481_507_3,
    -0.341_358_098_082_715_4,
    -0.978_795_476_555_777_7,
    1.916_254_377_395_988_4,
    0.03344287339036693,
    0.02870443944957101,
    -0.140_674_866_331_706_8,
    1.348_914_181_413_793_7,
];
pub(crate) const LINEAR_REC2020_TO_XYZ_D65: [f64; 9] = [
    0.636_958_048_301_291_3,
    0.14461690358620838,
    0.16888097516417205,
    0.26270021201126703,
    0.677_998_071_518_871,
    0.05930171646986194,
    0.00000000000000000,
    0.028_072_693_049_087_5,
    1.060_985_057_710_790_9,
];
pub(crate) const XYZ_D65_TO_LINEAR_REC2020: [f64; 9] = [
    1.716_651_187_971_267_6,
    -0.355_670_783_776_392_4,
    -0.253_366_281_373_659_8,
    -0.666_684_351_832_489,
    1.616_481_236_634_939,
    0.01576854581391113,
    0.01763985744531091,
    -0.04277061325780865,
    0.942_103_121_235_474,
];
pub(crate) const LINEAR_REC2020_TO_LMS: [f64; 9] = [
    0.616_755_784_865_444_4,
    0.36019840122646335,
    0.02304581390809228,
    0.265_133_059_392_636_7,
    0.635_839_372_067_849_1,
    0.09902756853951408,
    0.10010262952034828,
    0.20390652261661452,
    0.695_990_847_863_037_2,
];
pub(crate) const LMS_TO_LINEAR_REC2020: [f64; 9] = [
    2.139_906_730_434_651_3,
    -1.246_389_493_760_618,
    0.10648276332596668,
    -0.884_735_835_757_767_4,
    2.163_230_938_361_200_7,
    -0.278_495_102_603_433_4,
    -0.04857374640044396,
    -0.454_503_149_714_096_4,
    1.503_076_896_114_540_4,
];
pub(crate) const LINEAR_REC2020_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    0.835_187_333_129_723_5,
    0.04886884858605698,
    0.11594381828421951,
    0.05403324519953363,
    0.928_918_408_569_204_4,
    0.01704834623126199,
    -0.00234203897072539,
    0.03633215316169465,
    0.966_009_885_809_030_7,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_LINEAR_REC2020: [f64; 9] = [
    1.200_659_329_517_408,
    -0.05756805370122346,
    -0.14309127581618444,
    -0.06994154955888504,
    1.080_617_897_597_214,
    -0.01067634803832895,
    0.00554147334294746,
    -0.04078219298657951,
    1.035_240_719_643_632,
];
pub(crate) const LINEAR_REC2020_TO_XYZ_D50: [f64; 9] = [
    0.673_515_463_188_276,
    0.16569726370390453,
    0.12508294953738705,
    0.279_059_005_141_120_6,
    0.675_318_005_749_109_8,
    0.04562298910976962,
    -0.00193242713400438,
    0.02997782679282923,
    0.797_059_202_851_635_5,
];
pub(crate) const XYZ_D50_TO_LINEAR_REC2020: [f64; 9] = [
    1.647_184_904_671_766,
    -0.393_681_898_131_647_1,
    -0.23595963848828266,
    -0.682_664_107_417_381_8,
    1.647_714_612_744_407_6,
    0.01281708338512084,
    0.02966887665275675,
    -0.062_925_896_429_700_3,
    1.253_557_820_186_577_1,
];
pub(crate) const XYZ_D65_TO_LMS: [f64; 9] = [
    0.819_022_437_996_703,
    0.36190626005289034,
    -0.12887378152098788,
    0.03298365393238846,
    0.929_286_861_586_343_3,
    0.03614466635064235,
    0.048_177_189_359_624_2,
    0.264_239_531_752_730_8,
    0.633_547_828_469_430_8,
];
pub(crate) const LMS_TO_XYZ_D65: [f64; 9] = [
    1.226_879_875_845_924_3,
    -0.557_814_994_460_217_1,
    0.281_391_045_665_964_6,
    -0.04057574521480084,
    1.112_286_803_280_317_3,
    -0.07171105806551635,
    -0.07637293667466007,
    -0.42149333240224324,
    1.586_924_019_836_781_8,
];
pub(crate) const XYZ_D65_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    1.403_190_463_377_497_9,
    -0.22301514479051668,
    -0.101_606_685_074_137_9,
    -0.526_238_402_163_307_2,
    1.481_631_962_923_464_4,
    0.01701879027252688,
    -0.011_202_265_286_221_5,
    0.01824640347962099,
    0.911_247_227_491_504_8,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_XYZ_D65: [f64; 9] = [
    0.755_590_742_296_921,
    0.11271984265940525,
    0.082_145_342_095_345_4,
    0.268_321_843_578_571_9,
    0.715_115_256_661_791_2,
    0.01656289975963685,
    0.00391597276242580,
    -0.01293344283684181,
    1.098_075_220_834_294_5,
];
pub(crate) const XYZ_D65_TO_XYZ_D50: [f64; 9] = [
    1.047_929_792_544_996_6,
    0.02294687060160952,
    -0.05019226628920519,
    0.02962780877005567,
    0.990_434_426_753_88,
    -0.01707379906341879,
    -0.00924304064620452,
    0.01505519149029816,
    0.751_874_281_428_137,
];
pub(crate) const XYZ_D50_TO_XYZ_D65: [f64; 9] = [
    0.955_473_421_488_075_2,
    -0.02309845494876452,
    0.06325924320057065,
    -0.02836970933386358,
    1.009_995_398_081_304_1,
    0.021_041_441_191_917_3,
    0.01231401486448199,
    -0.02050764929889898,
    1.330_365_926_242_124,
];
pub(crate) const LMS_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    1.738_355_148_115_720_7,
    -0.987_950_942_751_445_8,
    0.24959579463572504,
    -0.707_049_401_532_926_6,
    1.934_370_044_440_138_2,
    -0.227_320_642_907_211_5,
    -0.08407882206239634,
    -0.35754060521141334,
    1.441_619_427_273_809_7,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_LMS: [f64; 9] = [
    0.715_448_460_565_553_4,
    0.35279155007721186,
    -0.068_240_010_642_765_3,
    0.274_411_649_001_567_1,
    0.667_797_649_841_236_7,
    0.05779070115719616,
    0.10978443261622942,
    0.18619829115002018,
    0.704_017_276_233_750_4,
];
pub(crate) const LMS_TO_XYZ_D50: [f64; 9] = [
    1.288_586_218_172_706,
    -0.537_871_744_497_374_5,
    0.213_581_202_754_236_4,
    -0.00253387643187372,
    1.092_316_798_871_916_5,
    -0.08978292244004273,
    -0.06937382305734124,
    -0.29500839894431263,
    1.189_486_824_512_114_2,
];
pub(crate) const XYZ_D50_TO_LMS: [f64; 9] = [
    0.770_700_042_043_117_2,
    0.34924840261939616,
    -0.11202351884164681,
    0.00559649248368848,
    0.937_072_340_113_676_9,
    0.06972568836252771,
    0.04633714262191069,
    0.25277531574310524,
    0.851_458_076_746_796,
];
pub(crate) const LINEAR_PROPHOTO_RGB_TO_XYZ_D50: [f64; 9] = [
    0.797_766_644_900_642_3,
    0.13518129740053308,
    0.031_347_734_128_392_2,
    0.288_074_828_819_401_3,
    0.711_835_234_241_873,
    0.00008993693872564,
    0.00000000000000000,
    0.00000000000000000,
    0.825_104_602_510_460_2,
];
pub(crate) const XYZ_D50_TO_LINEAR_PROPHOTO_RGB: [f64; 9] = [
    1.345_786_881_647_158_3,
    -0.25557208737979464,
    -0.05110186497554526,
    -0.544_630_705_124_901_9,
    1.508_247_742_845_146_8,
    0.02052744743642139,
    0.00000000000000000,
    0.00000000000000000,
    1.211_967_545_638_945_2,
];

// Multiplies a row-major 3x3 matrix by a 3-element column vector.
pub(crate) fn matrix_mul(m: &[f64; 9], v0: f64, v1: f64, v2: f64) -> (f64, f64, f64) {
    (
        (m[0] * v0) + (m[1] * v1) + (m[2] * v2),
        (m[3] * v0) + (m[4] * v1) + (m[5] * v2),
        (m[6] * v0) + (m[7] * v1) + (m[8] * v2),
    )
}

pub(crate) struct SrgbConvertOpts {
    pub missing_lightness: bool,
    pub missing_chroma: bool,
    pub missing_hue: bool,
}

// Missing-channel flags threaded through the linear pipeline.
//
// Matches Dart: the named `missing*` parameters of `ColorSpace.convertLinear`
// in `space.dart`; a channel that was missing in the source stays missing in
// the result rather than picking up a computed value.
pub(crate) struct ConvertLinearOpts {
    pub missing_lightness: bool,
    pub missing_chroma: bool,
    pub missing_hue: bool,
    pub missing_a: bool,
    pub missing_b: bool,
}

// The default conversion pipeline: linearize, transform, un-linearize,
// then hand polar destinations off to their dedicated space conversion.
//
// Matches Dart: `ColorSpace.convertLinear` (`space.dart`); missing inputs
// stay missing in the output.
pub(crate) fn convert_linear(
    src: ColorSpace,
    dst: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
    opts: Option<&ConvertLinearOpts>,
) -> SassResult<SassColor> {
    if dst == src {
        return Ok(new_color_for_space_internal_no_check(
            dst, c0, c1, c2, alpha,
        ));
    }

    // Normalize analogous sets of channels so that the destination color
    // for any space has its analogous channels set to missing when the
    // source color's channels are missing (#2810).
    let missing_lightness = opts.is_some_and(|o| o.missing_lightness);
    let mut missing_chroma = opts.is_some_and(|o| o.missing_chroma);
    let mut missing_hue = opts.is_some_and(|o| o.missing_hue);
    let mut missing_a = opts.is_some_and(|o| o.missing_a);
    let mut missing_b = opts.is_some_and(|o| o.missing_b);
    if missing_a && missing_b {
        missing_chroma = true;
        missing_hue = true;
    } else if missing_chroma && missing_hue {
        missing_a = true;
        missing_b = true;
    }
    if (missing_lightness && missing_chroma && missing_hue)
        || (c0.is_none() && c1.is_none() && c2.is_none())
    {
        return Ok(new_color_for_space_internal_no_check(
            dst, None, None, None, alpha,
        ));
    }

    let c0v = c0.unwrap_or(0.0);
    let c1v = c1.unwrap_or(0.0);
    let c2v = c2.unwrap_or(0.0);

    let (tr, tg, tb) = compute_linear_transform(src, dst, c0v, c1v, c2v);

    match dst {
        ColorSpace::Hsl | ColorSpace::Hwb => {
            let s_opts = SrgbConvertOpts {
                missing_lightness,
                missing_chroma,
                missing_hue,
            };
            srgb_convert_internal(dst, Some(tr), Some(tg), Some(tb), alpha, Some(&s_opts))
        }
        ColorSpace::Lab | ColorSpace::Lch => {
            let o = XyzD50ConvertOpts {
                missing_lightness,
                missing_chroma,
                missing_hue,
                missing_a,
                missing_b,
            };
            xyz_d50_convert_internal(dst, Some(tr), Some(tg), Some(tb), alpha, Some(&o))
        }
        ColorSpace::Oklab | ColorSpace::Oklch => {
            let o = LmsConvertOpts {
                missing_lightness,
                missing_chroma,
                missing_hue,
                missing_a,
                missing_b,
            };
            lms_convert_internal(dst, Some(tr), Some(tg), Some(tb), alpha, Some(&o))
        }
        _ => {
            let rp = c0.map(|_| tr);
            let gp = c1.map(|_| tg);
            let bp = c2.map(|_| tb);
            Ok(new_color_for_space_internal_no_check(
                dst, rp, gp, bp, alpha,
            ))
        }
    }
}

// Linearizes the source channels, multiplies by the source-to-destination
// matrix, then un-linearizes into the linear-destination form.
//
// Matches Dart: the matrix half of `ColorSpace.convertLinear`; the missing
// inputs default to zero here and are restored as missing by the caller.
fn compute_linear_transform(
    src: ColorSpace,
    dst: ColorSpace,
    c0v: f64,
    c1v: f64,
    c2v: f64,
) -> (f64, f64, f64) {
    let linear_dest = match dst {
        ColorSpace::Hsl | ColorSpace::Hwb => ColorSpace::Srgb,
        ColorSpace::Lab | ColorSpace::Lch => ColorSpace::XyzD50,
        ColorSpace::Oklab | ColorSpace::Oklch => ColorSpace::Lms,
        _ => dst,
    };
    if linear_dest == src {
        (c0v, c1v, c2v)
    } else {
        let lin_r = src.to_linear(c0v);
        let lin_g = src.to_linear(c1v);
        let lin_b = src.to_linear(c2v);
        let mat = src.transformation_matrix(linear_dest);
        let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let (r, g, b) = matrix_mul(mat.unwrap_or(&identity), lin_r, lin_g, lin_b);
        (
            linear_dest.from_linear(r),
            linear_dest.from_linear(g),
            linear_dest.from_linear(b),
        )
    }
}

// Dispatches to the source space's `convert`, splitting the missing-bitmask
// into per-channel `Option`s.
//
// Matches Dart: virtual `ColorSpace.convert`; the one `dst == src` fast path
// below mirrors `convertLinear`'s identity branch, while per-space identity
// handling lives in the individual `color_space_*.rs` conversions.
// Arity mirrors Dart's virtual `ColorSpace.convert`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_color(
    src: ColorSpace,
    dst: ColorSpace,
    c0: f64,
    c1: f64,
    c2: f64,
    alpha: f64,
    missing: [bool; 4],
    _extra_missing: Option<bool>,
) -> SassResult<SassColor> {
    let c0p = if missing[0] { None } else { Some(c0) };
    let c1p = if missing[1] { None } else { Some(c1) };
    let c2p = if missing[2] { None } else { Some(c2) };
    let ap = if missing[3] { None } else { Some(alpha) };

    use super::*;
    match src {
        ColorSpace::Rgb => color_space_rgb::rgb_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Hsl => color_space_hsl::hsl_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Hwb => color_space_hwb::hwb_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Srgb => color_space_srgb::srgb_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::SrgbLinear => {
            color_space_srgb_linear::srgb_linear_convert(dst, c0p, c1p, c2p, ap)
        }
        ColorSpace::DisplayP3 => color_space_display_p3::display_p3_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::DisplayP3Linear => {
            color_space_display_p3_linear::display_p3_linear_convert(dst, c0p, c1p, c2p, ap)
        }
        ColorSpace::A98Rgb => color_space_a98_rgb::a98_rgb_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::ProphotoRgb => {
            color_space_prophoto_rgb::prophoto_rgb_convert(dst, c0p, c1p, c2p, ap)
        }
        ColorSpace::Rec2020 => color_space_rec2020::rec2020_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::XyzD65 => color_space_xyz_d65::xyz_d65_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::XyzD50 => color_space_xyz_d50::xyz_d50_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Lab => color_space_lab::lab_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Lch => color_space_lch::lch_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Oklab => color_space_oklab::oklab_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Oklch => color_space_oklch::oklch_convert(dst, c0p, c1p, c2p, ap),
        ColorSpace::Lms => color_space_lms::lms_convert(dst, c0p, c1p, c2p, ap),
    }
}

pub(crate) struct XyzD50ConvertOpts {
    pub missing_lightness: bool,
    pub missing_chroma: bool,
    pub missing_hue: bool,
    pub missing_a: bool,
    pub missing_b: bool,
}
pub(crate) struct LabConvertOpts {
    pub missing_chroma: bool,
    pub missing_hue: bool,
}
pub(crate) struct OklabConvertOpts {
    pub missing_chroma: bool,
    pub missing_hue: bool,
}
pub(crate) struct LmsConvertOpts {
    pub missing_lightness: bool,
    pub missing_chroma: bool,
    pub missing_hue: bool,
    pub missing_a: bool,
    pub missing_b: bool,
}

// Does a partial conversion of a single XYZ component to Lab f-form.
//
// Matches Dart: `XyzD50ColorSpace._convertComponentToLabF` using the
// `labKappa`/`labEpsilon` constants from `space/utils.dart`.
pub(crate) fn lab_convert_component_to_f(component: f64) -> f64 {
    if component > LAB_EPSILON {
        math::pow(component, 1.0 / 3.0) + 0.0
    } else {
        ((LAB_KAPPA * component) + 16.0) / 116.0
    }
}

// Converts an f-form component back to the X or Z channel of an XYZ color.
//
// Matches Dart: `LabColorSpace._convertFToXorZ` (`space/lab.dart`).
pub(crate) fn lab_f_to_xz(component: f64) -> f64 {
    let cubed = component * component * component;
    if cubed > LAB_EPSILON {
        cubed
    } else {
        ((116.0 * component) - 16.0) / LAB_KAPPA
    }
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorSpace;
    use super::*;
    use crate::util::number;

    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_d50() {
        let d = &*D50;
        assert_eq!(ws(d[0]), "0.9642956764");
        assert_eq!(ws(d[1]), "1");
        assert_eq!(ws(d[2]), "0.8251046025");
    }

    #[test]
    fn test_matrix_mul() {
        let m = &LINEAR_SRGB_TO_XYZ_D65;
        let (r, g, b) = matrix_mul(m, 1.0, 0.0, 0.0);
        assert_eq!(ws(r), "0.4123907993");
        assert_eq!(ws(g), "0.2126390059");
        assert_eq!(ws(b), "0.0193308187");
        let (r, g, b) = matrix_mul(m, 0.0, 1.0, 0.0);
        assert_eq!(ws(r), "0.3575843394");
        assert_eq!(ws(g), "0.7151686788");
        assert_eq!(ws(b), "0.1191947798");
        let (r, g, b) = matrix_mul(m, 0.0, 0.0, 1.0);
        assert_eq!(ws(r), "0.1804807884");
        assert_eq!(ws(g), "0.0721923154");
        assert_eq!(ws(b), "0.9505321522");
    }

    #[test]
    fn test_convert_color() {
        let c = convert_color(
            ColorSpace::Srgb,
            ColorSpace::Lab,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "54.2905414047");
        assert_eq!(ws(c.channel1), "80.8049281704");
        assert_eq!(ws(c.channel2), "69.8909647686");
        let c = convert_color(
            ColorSpace::Srgb,
            ColorSpace::Lch,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "54.2905414047");
        assert_eq!(ws(c.channel1), "106.8371816032");
        assert_eq!(ws(c.channel2), "40.857656505");
        let c = convert_color(
            ColorSpace::Srgb,
            ColorSpace::Oklch,
            0.0,
            1.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0.8664396175");
        assert_eq!(ws(c.channel1), "0.2948272245");
        assert_eq!(ws(c.channel2), "142.4953450414");
        let c = convert_color(
            ColorSpace::Lab,
            ColorSpace::Lch,
            50.0,
            25.0,
            -25.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert_eq!(ws(c.channel1), "35.3553390593");
        assert_eq!(ws(c.channel2), "315");
        let c = convert_color(
            ColorSpace::Srgb,
            ColorSpace::Srgb,
            0.3,
            0.6,
            0.9,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0.3");
        assert_eq!(ws(c.channel1), "0.6");
        assert_eq!(ws(c.channel2), "0.9");
    }
}
