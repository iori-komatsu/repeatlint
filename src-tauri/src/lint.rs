use std::cell::OnceCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufReader, BufWriter, Cursor, Write};
use std::path::Path;
use std::sync::Mutex;
use std::{collections::HashMap, fs::File, io::Read, ops::Range};

use anyhow::{bail, Context};
use serde::Deserialize;
use vibrato::{dictionary::WordIdx, Dictionary, Tokenizer};

static HTML_HEAD: &'static str = r#"<div class="novel-body">"#;

static HTML_FOOT: &'static str = r#"</div>"#;

#[derive(Debug, Deserialize, PartialEq, Eq, Hash)]
struct Config {
    distance_threshold: usize,
    proper_nouns: Vec<String>,
    ignore_pos: Vec<String>,
    ignore_words: Vec<String>,
}

#[derive(Debug)]
struct Annotation {
    distance: usize,
}

fn read_user_dict(config: &Config) -> anyhow::Result<impl Read> {
    let mut buf = String::new();
    for noun in &config.proper_nouns {
        buf.push_str(&format!("{},1293,1293,0,固有名詞\n", noun));
    }
    buf.truncate(buf.trim_end().len());
    Ok(BufReader::new(Cursor::new(buf)))
}

fn create_tokenizer(exe_dir: &Path, config: &Config) -> anyhow::Result<Tokenizer> {
    let dict_path = exe_dir.join("dict/bccwj-suw+unidic-cwj-3_1_1+compact/system.dic.zst");
    let reader = zstd::Decoder::new(File::open(&dict_path).with_context(|| {
        format!(
            "辞書ファイル({})を開けませんでした。",
            dict_path.to_string_lossy()
        )
    })?)?;
    let mut dict = Dictionary::read(reader)?;
    let user_dict = read_user_dict(config)?;
    dict = dict.reset_user_lexicon_from_reader(Some(user_dict))?;
    let tokenizer = Tokenizer::new(dict).max_grouping_len(24);
    Ok(tokenizer)
}

fn calculate_hash<T: Hash>(t: &T) -> u64 {
    let mut s = DefaultHasher::new();
    t.hash(&mut s);
    s.finish()
}

pub fn lint(input: &str, project_root: Option<&Path>) -> anyhow::Result<String> {
    let exe_dir = match project_root {
        Some(p) => p.to_owned(),
        None => {
            let exe_path = std::env::current_exe()?;
            let Some(exe_dir) = exe_path.parent() else {
                bail!("failed to get exe directory");
            };
            exe_dir.to_owned()
        }
    };

    let config_path = exe_dir.join("config.toml");
    let config: Config = {
        let mut file =
            File::open(config_path).context("設定ファイル(config.toml)を開けませんでした")?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        toml::from_str(&buf)?
    };

    // tokenizer を毎回作成すると遅いので、キャッシュしておく。
    // config のハッシュ値が変わったときだけリロードする。
    static TOKENIZER_CACHE: Mutex<OnceCell<(u64, Tokenizer)>> = Mutex::new(OnceCell::new());
    let mut tokenizer_cell = TOKENIZER_CACHE.lock().unwrap();
    let current_hash = calculate_hash(&config);
    match tokenizer_cell.take() {
        Some((hash, tokenizer)) if hash == current_hash => {
            tokenizer_cell
                .set((hash, tokenizer))
                .map_err(|_| "set() must succeed")
                .unwrap();
        }
        _ => {
            let new_tokenizer = create_tokenizer(&exe_dir, &config)?;
            let new_hash = calculate_hash(&config);
            tokenizer_cell
                .set((new_hash, new_tokenizer))
                .map_err(|_| "set() must succeed")
                .unwrap();
        }
    };
    let tokenizer = &tokenizer_cell.get().unwrap().1;
    let mut worker = tokenizer.new_worker();

    worker.reset_sentence(input);
    worker.tokenize();

    println!("Tokenized. num_tokens={}", worker.num_tokens());

    // 転置インデックス
    let mut index: HashMap<WordIdx, Vec<Range<usize>>> = HashMap::new();
    let mut surfaces: HashMap<WordIdx, String> = HashMap::new();

    // テキストをスキャンして転置インデックスに入れていく
    for token in worker.token_iter() {
        let pos = match token.feature().split_once(',') {
            Some((pos, _)) => pos,
            None => token.feature(),
        };
        if config.ignore_pos.iter().any(|s| s == pos) {
            continue;
        }
        if config.ignore_words.iter().any(|w| w == token.surface()) {
            continue;
        }
        index
            .entry(token.word_idx())
            .or_default()
            .push(token.range_char());
        surfaces
            .entry(token.word_idx())
            .or_insert(token.surface().to_owned());
    }

    let mut annotations: HashMap<usize, Annotation> = HashMap::new();

    // 転置インデックスをスキャンして近くに同じ単語が出現していないか調べる
    for (_, ranges) in index.iter() {
        //let surface = &surfaces[word_idx];
        //let features = tokenizer.dictionary().word_feature(*word_idx);
        for i in 0..ranges.len() {
            let ri = &ranges[i];
            let mut min_dist = usize::MAX;
            for j in 0..ranges.len() {
                if i == j {
                    continue;
                }
                let rj = &ranges[j];
                let dist = usize::min(
                    usize::abs_diff(ri.start, rj.end),
                    usize::abs_diff(ri.end, rj.start),
                );
                min_dist = min_dist.min(dist);
            }
            annotations.insert(ri.start, Annotation { distance: min_dist });
        }
    }

    // HTMLを出力する
    let mut writer = BufWriter::new(Cursor::new(Vec::new()));
    write!(writer, "{}", HTML_HEAD)?;

    for token in worker.token_iter() {
        let annotation = annotations.get(&token.range_char().start);
        let class = if annotation.is_some_and(|a| a.distance < config.distance_threshold) {
            "alert"
        } else {
            ""
        };
        write!(
            writer,
            "<span title='{}' class='{}'>",
            html_escape::encode_text(token.feature()),
            class
        )?;
        write!(writer, "{}", html_escape::encode_text(token.surface()))?;
        write!(writer, "</span>")?;
        write!(writer, "<span class='separator'>|</span>")?;
    }

    write!(writer, "{}", HTML_FOOT)?;
    let html = writer.into_inner()?.into_inner();
    Ok(String::from_utf8_lossy(&html).to_string())
}
