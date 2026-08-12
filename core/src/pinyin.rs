#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Syllable(pub String);

pub type SegmentPath = Vec<Syllable>;

static VALID_SYLLABLES: &[&str] = &[

    "a", "o", "e", "ai", "ei", "ao", "ou", "an", "en", "ang", "eng", "er",

    "ba","bo","bai","bei","bao","ban","ben","bang","beng","bi","bie","biao",
    "bian","bin","bing","bu",

    "pa","po","pai","pei","pao","pou","pan","pen","pang","peng","pi","pie",
    "piao","pian","pin","ping","pu",

    "ma","mo","me","mai","mei","mao","mou","man","men","mang","meng","mi",
    "mie","miao","miu","mian","min","ming","mu",

    "fa","fo","fei","fou","fan","fen","fang","feng","fu",

    "da","de","dai","dei","dao","dou","dan","den","dang","deng","dong","di",
    "die","diao","diu","dian","ding","du","duan","dui","dun","duo",

    "ta","te","tai","tao","tou","tan","tang","teng","tong","ti","tie","tiao",
    "tian","ting","tu","tuan","tui","tun","tuo",

    "na","ne","nai","nei","nao","nou","nan","nen","nang","neng","nong","ni",
    "nie","niao","niu","nian","nin","niang","ning","nu","nuan","nun","nuo","nv","nve",

    "la","le","lai","lei","lao","lou","lan","lang","leng","long","li","lia",
    "lie","liao","liu","lian","lin","liang","ling","lu","luan","lun","luo","lv","lve",

    "ga","ge","gai","gei","gao","gou","gan","gen","gang","geng","gong","gu",
    "gua","guai","gui","guan","gun","guo","guang",

    "ka","ke","kai","kei","kao","kou","kan","ken","kang","keng","kong","ku",
    "kua","kuai","kui","kuan","kun","kuo","kuang",

    "ha","he","hai","hei","hao","hou","han","hen","hang","heng","hong","hu",
    "hua","huai","hui","huan","hun","huo","huang",

    "ji","jia","jie","jiao","jiu","jian","jin","jiang","jing","jiong","ju","juan","jun","jue",

    "qi","qia","qie","qiao","qiu","qian","qin","qiang","qing","qiong","qu","quan","qun","que",

    "xi","xia","xie","xiao","xiu","xian","xin","xiang","xing","xiong","xu","xuan","xun","xue",

    "zha","zhe","zhi","zhai","zhao","zhou","zhan","zhen","zhang","zheng","zhong","zhu",
    "zhua","zhuai","zhui","zhuan","zhun","zhuo","zhuang",

    "cha","che","chi","chai","chao","chou","chan","chen","chang","cheng","chong","chu",
    "chuai","chui","chuan","chun","chuo","chuang",

    "sha","she","shi","shai","shao","shou","shan","shen","shang","sheng","shu",
    "shua","shuai","shui","shuan","shun","shuo","shuang",

    "re","ri","rao","rou","ran","ren","rang","reng","rong","ru","ruan","rui","run","ruo",

    "za","ze","zi","zai","zao","zou","zan","zen","zang","zeng","zong","zu","zuan","zui","zun","zuo",

    "ca","ce","ci","cai","cao","cou","can","cen","cang","ceng","cong","cu","cuan","cui","cun","cuo",

    "sa","se","si","sai","sao","sou","san","sen","sang","seng","song","su","suan","sui","sun","suo",

    "ya","yo","ye","yi","yao","you","yan","yin","yang","ying","yong","yu","yuan","yun","yue",

    "wa","wo","wei","wai","wan","wen","wang","weng","wu",
];

pub fn segment(raw: &str) -> Vec<SegmentPath> {
    let s = raw.to_lowercase();
    let n = s.len();
    if n == 0 {
        return vec![];
    }

    const MAX_SYLLABLE_LEN: usize = 6;
    const MAX_PATHS: usize = 8;

    let bytes = s.as_bytes();

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

    result.sort_by_key(|p| p.len());
    result
}

pub fn is_valid_syllable(s: &str) -> bool {
    VALID_SYLLABLES.contains(&s)
}

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

        assert!(is_valid_syllable("lv"));
        assert!(is_valid_syllable("nv"));
        assert!(is_valid_syllable("lve"));
        assert!(is_valid_syllable("nve"));
    }

    #[test]
    fn test_segment_zhuang() {

        let paths = segment("zhuang");
        assert!(paths.iter().any(|p| p == &vec![Syllable("zhuang".into())]));
    }
}
