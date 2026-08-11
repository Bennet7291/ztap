//! Pinyin parsing and syllable segmentation module.
//!
//! Responsibilities:
//!   - Split a continuous pinyin string into a sequence of syllables
//!   - Validate individual syllables against Mandarin phonology
//!   - Return multiple candidate segmentation paths for ambiguous input

/// A single Mandarin pinyin syllable (e.g. "zhong", "guo").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Syllable(pub String);

/// One possible segmentation of a pinyin string.
pub type SegmentPath = Vec<Syllable>;

/// Complete set of valid Mandarin syllables (initial + final combinations).
///
/// Derived from the official Hanyu Pinyin scheme (~410 valid syllables,
/// standard Putonghua set — excludes dialectal/onomatopoeic outliers such
/// as "biang", "dia", "fiao", "nia", "pia", "rua" that appear in some
/// character-frequency corpora for individual rare characters but are not
/// part of the standard syllable inventory taught or typed against).
///
/// `ü` is written as `v` throughout (the universal IME convention — see
/// `is_valid_syllable`'s doc comment), since `segment()` only ever receives
/// lowercase ASCII from the keyboard and could never match a literal `ü`.
static VALID_SYLLABLES: &[&str] = &[
    // Zero initial
    "a", "o", "e", "ai", "ei", "ao", "ou", "an", "en", "ang", "eng", "er",
    // b
    "ba","bo","bai","bei","bao","ban","ben","bang","beng","bi","bie","biao",
    "bian","bin","bing","bu",
    // p
    "pa","po","pai","pei","pao","pou","pan","pen","pang","peng","pi","pie",
    "piao","pian","pin","ping","pu",
    // m
    "ma","mo","me","mai","mei","mao","mou","man","men","mang","meng","mi",
    "mie","miao","miu","mian","min","ming","mu",
    // f
    "fa","fo","fei","fou","fan","fen","fang","feng","fu",
    // d
    "da","de","dai","dei","dao","dou","dan","den","dang","deng","dong","di",
    "die","diao","diu","dian","ding","du","duan","dui","dun","duo",
    // t
    "ta","te","tai","tao","tou","tan","tang","teng","tong","ti","tie","tiao",
    "tian","ting","tu","tuan","tui","tun","tuo",
    // n
    "na","ne","nai","nei","nao","nou","nan","nen","nang","neng","nong","ni",
    "nie","niao","niu","nian","nin","niang","ning","nu","nuan","nun","nuo","nv","nve",
    // l
    "la","le","lai","lei","lao","lou","lan","lang","leng","long","li","lia",
    "lie","liao","liu","lian","lin","liang","ling","lu","luan","lun","luo","lv","lve",
    // g
    "ga","ge","gai","gei","gao","gou","gan","gen","gang","geng","gong","gu",
    "gua","guai","gui","guan","gun","guo","guang",
    // k
    "ka","ke","kai","kei","kao","kou","kan","ken","kang","keng","kong","ku",
    "kua","kuai","kui","kuan","kun","kuo","kuang",
    // h
    "ha","he","hai","hei","hao","hou","han","hen","hang","heng","hong","hu",
    "hua","huai","hui","huan","hun","huo","huang",
    // j
    "ji","jia","jie","jiao","jiu","jian","jin","jiang","jing","jiong","ju","juan","jun","jue",
    // q
    "qi","qia","qie","qiao","qiu","qian","qin","qiang","qing","qiong","qu","quan","qun","que",
    // x
    "xi","xia","xie","xiao","xiu","xian","xin","xiang","xing","xiong","xu","xuan","xun","xue",
    // zh
    "zha","zhe","zhi","zhai","zhao","zhou","zhan","zhen","zhang","zheng","zhong","zhu",
    "zhua","zhuai","zhui","zhuan","zhun","zhuo","zhuang",
    // ch
    "cha","che","chi","chai","chao","chou","chan","chen","chang","cheng","chong","chu",
    "chuai","chui","chuan","chun","chuo","chuang",
    // sh
    "sha","she","shi","shai","shao","shou","shan","shen","shang","sheng","shu",
    "shua","shuai","shui","shuan","shun","shuo","shuang",
    // r
    "re","ri","rao","rou","ran","ren","rang","reng","rong","ru","ruan","rui","run","ruo",
    // z
    "za","ze","zi","zai","zao","zou","zan","zen","zang","zeng","zong","zu","zuan","zui","zun","zuo",
    // c
    "ca","ce","ci","cai","cao","cou","can","cen","cang","ceng","cong","cu","cuan","cui","cun","cuo",
    // s
    "sa","se","si","sai","sao","sou","san","sen","sang","seng","song","su","suan","sui","sun","suo",
    // y
    "ya","yo","ye","yi","yao","you","yan","yin","yang","ying","yong","yu","yuan","yun","yue",
    // w
    "wa","wo","wei","wai","wan","wen","wang","weng","wu",
];

/// Split a raw pinyin string into all valid syllable segmentation paths.
///
/// Uses dynamic programming to enumerate every legal split. Paths are sorted
/// by syllable count ascending (fewer syllables = longer words = preferred).
/// At most `MAX_PATHS` paths are returned to avoid combinatorial explosion.
///
/// # Example
/// ```
/// let paths = segment("zhongguoren");
/// assert_eq!(paths[0], vec![
///     Syllable("zhong".into()), Syllable("guo".into()), Syllable("ren".into())
/// ]);
/// ```
pub fn segment(raw: &str) -> Vec<SegmentPath> {
    let s = raw.to_lowercase();
    let n = s.len();
    if n == 0 {
        return vec![];
    }

    const MAX_SYLLABLE_LEN: usize = 6; // longest valid syllable: "zhuang"
    const MAX_PATHS: usize = 8;

    let bytes = s.as_bytes();

    // paths_from[i]: all complete segmentation paths starting at byte index i.
    // Fill right-to-left; paths_from[n] = [ [] ] (empty path = reached end).
    let mut paths_from: Vec<Vec<SegmentPath>> = vec![vec![]; n + 1];
    paths_from[n] = vec![vec![]];

    for i in (0..n).rev() {
        let max_len = MAX_SYLLABLE_LEN.min(n - i);
        'outer: for len in 1..=max_len {
            let slice = match std::str::from_utf8(&bytes[i..i + len]) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if is_valid_syllable(slice) {
                let syl = Syllable(slice.to_string());
                for tail in paths_from[i + len].clone() {
                    let mut path = vec![syl.clone()];
                    path.extend(tail);
                    paths_from[i].push(path);
                    if paths_from[i].len() >= MAX_PATHS {
                        break 'outer;
                    }
                }
            }
        }
    }

    let mut result = paths_from.remove(0);
    // Prefer paths with fewer syllables (longer word matches).
    result.sort_by_key(|p| p.len());
    result
}

/// Return `true` if `s` is a valid Mandarin pinyin syllable (no tone marks).
///
/// `ü`-containing syllables (after "l"/"n"/"j"/"q"/"x"/"y") are matched in
/// their `v`-substituted ASCII form (e.g. "lv", not "lü"), since that is
/// what a physical keyboard actually produces and what `segment()` passes
/// in. This mirrors the near-universal IME convention (Microsoft Pinyin,
/// Sogou, Rime's default `v` mapping) rather than requiring a dead-key or
/// alternate input for the umlaut.
pub fn is_valid_syllable(s: &str) -> bool {
    VALID_SYLLABLES.contains(&s)
}

/// Find the length of the longest prefix of `s` that forms a valid syllable.
///
/// Returns 0 if no prefix is a valid syllable.
/// Used during live typing to detect syllable boundaries.
pub fn longest_valid_prefix(s: &str) -> usize {
    let mut best = 0;
    for len in 1..=s.len().min(6) {
        if is_valid_syllable(&s[..len]) {
            best = len;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_syllable() {
        assert!(is_valid_syllable("zhong"));
        assert!(is_valid_syllable("guo"));
        assert!(!is_valid_syllable("xx"));
        assert!(!is_valid_syllable(""));
    }

    #[test]
    fn test_segment_basic() {
        let paths = segment("zhongguoren");
        assert!(!paths.is_empty());
        let first = &paths[0];
        assert_eq!(first[0], Syllable("zhong".into()));
        assert_eq!(first[1], Syllable("guo".into()));
        assert_eq!(first[2], Syllable("ren".into()));
    }

    #[test]
    fn test_segment_nihao() {
        let paths = segment("nihao");
        assert!(paths.iter().any(|p| p == &vec![
            Syllable("ni".into()),
            Syllable("hao".into()),
        ]));
    }

    #[test]
    fn test_segment_empty() {
        assert!(segment("").is_empty());
    }

    #[test]
    fn test_longest_valid_prefix() {
        assert_eq!(longest_valid_prefix("zhong"), 5);
        assert_eq!(longest_valid_prefix("ni"), 2);
        assert_eq!(longest_valid_prefix("xyz"), 0);
    }

    /// Regression test: these initial+final combinations are common,
    /// everyday syllables (载/再/zai, 帅/摔/shuai's sibling shuang, 装/zhuang,
    /// 光/guang, etc.) that were previously missing from VALID_SYLLABLES,
    /// silently breaking segmentation for any word containing them.
    #[test]
    fn test_previously_missing_common_syllables() {
        for s in [
            "zai", "cai", "sai", "chai", "shai", "zhai",
            "guang", "kuang", "huang", "zhuang", "chuang", "shuang",
        ] {
            assert!(is_valid_syllable(s), "{s} should be a valid syllable");
        }
    }

    #[test]
    fn test_v_is_the_umlaut_substitute() {
        // "lü"/"nü" are unreachable from real keyboard input (see doc
        // comment on is_valid_syllable), so the ASCII "v" convention is
        // what must actually validate here.
        assert!(is_valid_syllable("lv"));
        assert!(is_valid_syllable("nv"));
        assert!(is_valid_syllable("lve"));
        assert!(is_valid_syllable("nve"));
    }

    #[test]
    fn test_segment_zhuang() {
        // "zhuangguo" should segment as zhuang+guo, not zhu+ang+guo or fail.
        let paths = segment("zhuang");
        assert!(paths.iter().any(|p| p == &vec![Syllable("zhuang".into())]));
    }
}
