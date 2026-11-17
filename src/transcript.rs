//! Offline transcript parsing, bounded local suggestions, and caption rendering.
//! This module has no HTTP client, inference backend, account, or API-key handling.
use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub(crate) const MAX_TEXT: usize = 2 * 1024 * 1024;
pub(crate) const MAX_CUES: usize = 10000;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Word { pub start:f64, pub end:f64, pub word:String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Cue { pub start:f64, pub end:f64, pub text:String, #[serde(default)] pub words:Vec<Word> }
#[derive(Clone)]
pub(crate) struct Document { pub id:String, pub format:String, pub text:String, pub cues:Vec<Cue>, pub source:Option<PathBuf> }
impl Document {
    pub fn end(&self)->f64{self.cues.iter().map(|c|c.end).fold(0.0,f64::max)}
    pub fn info(&self)->serde_json::Value {serde_json::json!({"id":self.id,"format":self.format,"timed":!self.cues.is_empty(),"cues":self.cues.len(),"duration":self.end(),"source":self.source,"text_url":format!("/media/{}.txt",self.id),"srt_url":if self.cues.is_empty(){None}else{Some(format!("/media/{}.transcript.srt",self.id))}})}
}
pub(crate) fn clean(s:&str)->String {
    let mut tag=false;let mut out=String::new();
    for c in s.chars(){match c {'<'=>tag=true,'>'=>tag=false,'{'|'}'|'\\'=>{},_ if !tag&&!c.is_control()&&!matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')=>out.push(c),_=>{}}}
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn timestamp(s:&str)->Result<f64>{
    let parts:Vec<_>=s.trim().split(':').collect();if !(2..=3).contains(&parts.len()){bail!("Use HH:MM:SS,mmm or MM:SS.mmm timestamps");}
    let last=parts[parts.len()-1].replace(',','.');let(sec,fraction)=last.split_once('.').context("Timestamp needs milliseconds")?;
    if fraction.is_empty()||fraction.len()>3||!fraction.bytes().all(|b|b.is_ascii_digit())||!sec.bytes().all(|b|b.is_ascii_digit()){bail!("Invalid timestamp milliseconds");}
    let sec:u32=sec.parse()?;let min:u32=parts[parts.len()-2].parse()?;let hour:u32=if parts.len()==3{parts[0].parse()?}else{0};
    if sec>=60||min>=60||hour>=48{bail!("Timestamp outside supported bounds");}
    Ok(hour as f64*3600.0+min as f64*60.0+sec as f64+fraction.parse::<u32>()? as f64/10u32.pow(fraction.len() as u32) as f64)
}
fn checked_cue(mut c:Cue)->Result<Cue>{
    if !c.start.is_finite()||!c.end.is_finite()||c.start<0.0||c.end<=c.start||c.end>172800.0||c.text.chars().count()>1200{bail!("Invalid cue timing or cue exceeds 1200 characters");}
    c.text=clean(&c.text);c.words.clear();Ok(c)
}
pub(crate) fn parse(format:&str,input:&str)->Result<Document>{
    if input.len()>MAX_TEXT||input.trim().is_empty(){bail!("Supply a nonempty transcript of at most 2 MiB");}
    let raw=input.trim_start_matches('\u{feff}').replace("\r\n","\n").replace('\r',"\n");
    let mut cues=vec![];
    match format {
        "txt"=>{},
        "srt"|"vtt"=>{
            let lines:Vec<_>=raw.lines().collect();let mut i=0;
            if format=="vtt"{if !lines.first().is_some_and(|l|l.starts_with("WEBVTT")){bail!("VTT must begin with WEBVTT");}i=1;while i<lines.len()&&!lines[i].trim().is_empty(){i+=1;}}
            while i<lines.len(){
                while i<lines.len()&&lines[i].trim().is_empty(){i+=1;}if i==lines.len(){break;}
                if format=="vtt"&&["NOTE","STYLE","REGION"].iter().any(|v|lines[i].starts_with(v)){while i<lines.len()&&!lines[i].trim().is_empty(){i+=1;}continue;}
                if !lines[i].contains("-->"){i+=1;}
                if i>=lines.len(){bail!("Missing cue timestamps");}
                let(left,right)=lines[i].split_once("-->").context("Invalid cue timing line")?;
                let start=timestamp(left)?;let end=timestamp(right.split_whitespace().next().context("Missing cue end")?)?;i+=1;
                let mut text=String::new();while i<lines.len()&&!lines[i].trim().is_empty(){if !text.is_empty(){text.push(' ');}text.push_str(lines[i]);i+=1;}
                let c=checked_cue(Cue{start,end,text,words:vec![]})?;if !c.text.is_empty(){cues.push(c);}if cues.len()>MAX_CUES{bail!("At most 10000 cues are supported per project");}
            }
        },
        "json"=>{
            let v:serde_json::Value=serde_json::from_str(&raw)?;
            if let Some(array)=v.get("words").and_then(|v|v.as_array()).filter(|v|!v.is_empty()){
                if array.len()>30000{bail!("At most 30000 real word timestamps are supported");}
                let mut words=Vec::with_capacity(array.len());
                for value in array{let mut w:Word=serde_json::from_value(value.clone())?;if !w.start.is_finite()||!w.end.is_finite()||w.start<0.0||w.end<=w.start||w.end>172800.0||w.word.chars().count()>1200{bail!("Invalid real word timestamp");}w.word=clean(&w.word);if !w.word.is_empty(){words.push(w);}}
                words.sort_by(|a,b|a.start.total_cmp(&b.start));
                for w in words{
                    if let Some(c)=cues.last_mut(){let c:&mut Cue=c;if c.words.len()<6&&c.text.chars().count()+w.word.chars().count()<42&&w.start-c.end<0.65&&w.end-c.start<3.0{c.text.push(' ');c.text.push_str(&w.word);c.end=c.end.max(w.end);c.words.push(w);continue;}}
                    cues.push(Cue{start:w.start,end:w.end,text:w.word.clone(),words:vec![w]});
                }
            }else{
                let array=v.get("segments").and_then(|v|v.as_array()).context("JSON needs segments or actual word timestamps")?;
                if array.len()>MAX_CUES{bail!("At most 10000 segments are supported");}
                for value in array{let c=checked_cue(serde_json::from_value(value.clone())?)?;if !c.text.is_empty(){cues.push(c);}}
            }
        },_=>bail!("Choose SRT, VTT, word-timed JSON, or plain TXT")
    }
    if format!="txt"&&cues.is_empty(){bail!("No usable timed captions were found");}
    cues.sort_by(|a,b|a.start.total_cmp(&b.start));
    let text=if format=="txt"{raw}else{cues.iter().map(|c|c.text.as_str()).collect::<Vec<_>>().join("\n")+"\n"};
    Ok(Document{id:id(),format:format.into(),text,cues,source:None})
}
#[derive(Clone,Serialize)]
pub(crate) struct Suggestion {pub start:f64,pub duration:f64,pub score:u32,pub title:String,pub excerpt:String,pub reason:String}
#[derive(Clone,Eq,PartialEq,Ord,PartialOrd)]
struct Pick {score:u32,start_ms:u64,end_ms:u64,first:usize,last:usize}
pub(crate) fn validate_focus(s:&str)->Result<()>{if !["best","funny","hype","tips"].contains(&s){bail!("Choose Best, Funny, Hype or Tips focus");}Ok(())}
pub(crate) fn suggest(doc:&Document,max:f64,focus:&str,query:&str)->Result<Vec<Suggestion>>{
    validate_focus(focus)?;if !(5.0..=60.0).contains(&max)||query.chars().count()>160{bail!("Invalid suggestion duration or search length");}
    if doc.cues.is_empty(){bail!("TXT has no timing. Import SRT/VTT or timed JSON for clip suggestions; timing is never invented.");}
    let terms:Vec<_>=query.split_whitespace().map(|s|s.to_lowercase()).collect();if terms.len()>8{bail!("Search for at most 8 words at once");}
    let lexicon:&[&str]=match focus{"funny"=>&["funny","joke","laugh","lol","haha","hilarious","oops","fail","मजाक","हंसी"],"hype"=>&["wow","insane","clutch","unbelievable","win","won","incredible","yes","वाह","जीत"],"tips"=>&["how","tip","because","first","steps","remember","avoid","learn","कैसे","सीखें"],_=>&[]};
    // Merge overlapping cues into indivisible blocks, then precompute text statistics once.
    let mut blocks:Vec<(f64,f64,usize,usize)>=vec![];
    for(i,c)in doc.cues.iter().enumerate(){if let Some(b)=blocks.last_mut(){if c.start<b.1-0.001{b.1=b.1.max(c.end);b.3=i;continue;}}blocks.push((c.start,c.end,i,i));}
    let stats:Vec<_>=blocks.iter().map(|b|{let text=doc.cues[b.2..=b.3].iter().map(|c|c.text.as_str()).collect::<Vec<_>>().join(" ");let lower=text.to_lowercase();let words=lower.split(|c:char|!c.is_alphanumeric()).filter(|s|!s.is_empty()).collect::<Vec<_>>();let count=words.len().max(text.chars().filter(|c|!c.is_ascii()).count()/3);let hits=words.iter().filter(|word|lexicon.contains(word)).count();let mask=terms.iter().enumerate().fold(0u16,|m,(i,t)|if lower.contains(t){m|(1<<i)}else{m});let sentence=text.ends_with(['.','!','?','।','。','！','？']);(count,hits,mask,sentence)}).collect();
    let needed=(1u16<<terms.len())-1;let mut heap:BinaryHeap<Reverse<Pick>>=BinaryHeap::new();
    for i in 0..blocks.len(){let mut words=0;let mut hits=0;let mut mask=0;
        for j in i..(i+80).min(blocks.len()){
            if j>i&&blocks[j].0-blocks[j-1].1>3.0{break;}
            let duration=blocks[j].1-blocks[i].0;if duration>max+0.0001{break;}
            words+=stats[j].0;hits+=stats[j].1;mask|=stats[j].2;
            if duration<5.0||words<6||mask!=needed||(focus!="best"&&hits==0){continue;}
            let density=(words as f64/duration).min(3.0);let score=(30.0+density*8.0+(duration/max)*10.0+if stats[j].3{8.0}else{0.0}+(hits.min(5)*7)as f64).round().clamp(0.0,100.0)as u32;
            let p=Pick{score,start_ms:(blocks[i].0*1000.0).round()as u64,end_ms:(blocks[j].1*1000.0).round()as u64,first:blocks[i].2,last:blocks[j].3};heap.push(Reverse(p));if heap.len()>192{heap.pop();}
        }
    }
    let mut picks=heap.into_iter().map(|p|p.0).collect::<Vec<_>>();picks.sort_by(|a,b|b.score.cmp(&a.score).then(a.start_ms.cmp(&b.start_ms)));
    let mut out:Vec<Suggestion>=vec![];
    for p in picks{let start=p.start_ms as f64/1000.0;let end=p.end_ms as f64/1000.0;
        if out.iter().any(|q|((q.start+q.duration).min(end)-q.start.max(start)).max(0.0)>(end-start).min(q.duration)*0.5){continue;}
        let text=doc.cues[p.first..=p.last].iter().map(|c|c.text.as_str()).collect::<Vec<_>>().join(" ");
        out.push(Suggestion{start,duration:end-start,score:p.score,title:doc.cues[p.first].text.chars().take(90).collect(),excerpt:text.chars().take(500).collect(),reason:format!("Local text rules · {focus} focus · complete caption blocks · not an AI/virality prediction")});if out.len()==12{break;}
    }Ok(out)
}
pub(crate) fn validate_cut(doc:&Document,start:f64,duration:f64,max:f64)->Result<()>{
    if !start.is_finite()||!duration.is_finite()||start<0.0||duration<5.0||duration>max+0.001||start+duration>doc.end()+0.03{bail!("Invalid transcript cut bounds");}
    let end=start+duration;
    if !doc.cues.iter().any(|c|(c.start-start).abs()<0.03)||!doc.cues.iter().any(|c|(c.end-end).abs()<0.03)||doc.cues.iter().any(|c|(c.start+0.03<start&&start<c.end-0.03)||(c.start+0.03<end&&end<c.end-0.03)){bail!("Cut must preserve complete transcript cue boundaries");}Ok(())
}
fn srt_time(s:f64)->String{let n=(s.max(0.0)*1000.0).round()as u64;format!("{:02}:{:02}:{:02},{:03}",n/3600000,(n/60000)%60,(n/1000)%60,n%1000)}
fn ass_time(s:f64)->String{let n=(s.max(0.0)*100.0).round()as u64;format!("{}:{:02}:{:02}.{:02}",n/360000,(n/6000)%60,(n/100)%60,n%100)}
fn cue_text(c:&Cue,offset:f64,duration:f64)->String{if c.words.is_empty(){return clean(&c.text);}c.words.iter().filter(|w|w.end>offset&&w.start<offset+duration).map(|w|clean(&w.word)).collect::<Vec<_>>().join(" ")}
pub(crate) fn srt(cues:&[Cue],offset:f64,duration:f64)->String{let mut out=String::new();let mut index=0;for c in cues{let l=(c.start-offset).max(0.0);let r=(c.end-offset).min(duration);if r-l<0.02{continue;}let text=cue_text(c,offset,duration);if text.is_empty(){continue;}index+=1;out.push_str(&format!("{index}\n{} --> {}\n{text}\n\n",srt_time(l),srt_time(r)));}out}
pub(crate) fn validate_style(style:&str,size:u32,position:&str)->Result<()>{if !["clean","bold","box","karaoke"].contains(&style)||![75,100,125].contains(&size)||!["top","center","bottom"].contains(&position){bail!("Invalid caption style, size or placement");}Ok(())}
pub(crate) fn ass(cues:&[Cue],offset:f64,duration:f64,w:u32,h:u32,style:&str,size:u32,position:&str)->String{
    let factor=if style=="bold"{0.072}else{0.06};let font=(w.min(h)as f64*factor*size as f64/100.0).round().clamp(18.0,120.0)as u32;let margin=(h as f64*0.12)as u32;let side=(w as f64*0.055)as u32;let alignment=match position{"top"=>8,"center"=>5,_=>2};
    let(primary,outline,border,thickness,shadow,bold)=match style{"bold"=>("&H0000D7FF","&H00101010",1,4,2,-1),"box"=>("&H00FFFFFF","&H80000000",3,8,0,0),"karaoke"=>("&H0000D7FF","&H00101010",1,3,1,-1),_=>("&H00FFFFFF","&H00101010",1,3,1,0)};
    let mut out=format!("[Script Info]\nScriptType: v4.00+\nPlayResX: {w}\nPlayResY: {h}\nScaledBorderAndShadow: yes\nWrapStyle: 0\n; Caption preset: {style}\n\n[V4+ Styles]\nFormat: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\nStyle: Default,Arial,{font},{primary},&H00FFFFFF,{outline},&H80000000,{bold},0,0,0,100,100,0,0,{border},{thickness},{shadow},{alignment},{side},{side},{margin},1\nStyle: Plain,Arial,{font},&H00FFFFFF,&H00FFFFFF,&H00101010,&H80000000,0,0,0,0,100,100,0,0,1,3,1,{alignment},{side},{side},{margin},1\n\n[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n");
    for c in cues{let l=(c.start-offset).max(0.0);let r=(c.end-offset).min(duration);if r-l<0.02{continue;}let words=c.words.iter().filter(|w|w.end>offset+l&&w.start<offset+r).collect::<Vec<_>>();
        let(event,body)=if style=="karaoke"&&!words.is_empty(){let mut b=String::new();let mut cursor=(l*100.0).round()as u64;for(i,word)in words.iter().enumerate(){let end=words.get(i+1).map(|v|v.start-offset).unwrap_or(r).clamp(l,r);let end_cs=((end*100.0).round()as u64).max(cursor);if i>0{b.push(' ');}b.push_str(&format!("{{\\k{}}}{}",end_cs-cursor,clean(&word.word)));cursor=end_cs;}("Default",b)}else{(if style=="karaoke"{"Plain"}else{"Default"},cue_text(c,offset,duration))};
        if !body.is_empty(){out.push_str(&format!("Dialogue: 0,{},{},{event},,0,0,0,,{body}\n",ass_time(l),ass_time(r)));}
    }out
}
pub(crate) fn synthetic()->Document{parse("json",r#"{"words":[{"start":0.05,"end":0.3,"word":"Your"},{"start":0.32,"end":0.6,"word":"PC"},{"start":0.62,"end":0.9,"word":"renders"},{"start":0.92,"end":1.2,"word":"captions"},{"start":2,"end":2.2,"word":"No"},{"start":2.22,"end":2.5,"word":"API"},{"start":2.52,"end":2.9,"word":"keys"}]}"#).expect("valid synthetic transcript")}
