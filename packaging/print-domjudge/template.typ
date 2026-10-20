#set page(paper: "a4", margin: 15mm)
#set document(title: "Natsume source print", author: "Natsume", date: none)
#set text(font: "Noto Sans Mono CJK SC", size: 9pt, fill: black)
#set par(justify: false, leading: 0.4em)
#let display = json("display.json")
#let seat = if display.seat_label == none { "未分配" } else { display.seat_label }
#text("队伍：" + display.team_name + "\n账号：" + display.account + "\n座位：" + seat)
#linebreak()
#text("文件：" + display.original_filename + "\n请求 ID：" + display.request_id)
#v(0.7em)
// Raw text is data, not Typst. Permit wrapping even inside long unbroken tokens.
#show raw: set text(ligatures: false)
#show raw: set block(width: 100%)
#show raw: set par(linebreaks: "simple")
#show raw: it => {
  let lines = it.text.split("\n")
  for (index, line) in lines.enumerate() {
    if index > 0 { linebreak() }
    if line == "" { text(" ") }
    else {
      // Zero-width break opportunities preserve every original character.
      let visible = line.replace("\t", "    ").replace(" ", "\u{a0}")
      text(visible.clusters().join("\u{200b}"))
    }
  }
}
#raw(read("source.txt"), block: true)
