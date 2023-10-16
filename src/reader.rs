use walkdir::{DirEntry, WalkDir};

pub struct ContentFile {
    pub origin: String,
    pub destination: String,
}

fn adjust_path_depth(path: &str, depth: usize, ext: &str) -> String {
    let clean_path: Vec<&str> = path.strip_suffix(ext).unwrap().split('/').collect();
    let (file_name, components) = clean_path.split_last().unwrap();
    let mut adjusted_path = String::new();
    if components.len() > depth - 1 {
        //shorten path
        let path = &components[0..depth - 1];
        adjusted_path.push_str(&path.join("/"));
        if path.len() > 0 {
            adjusted_path.push('/');
        }
        adjusted_path.push_str(file_name);
    } else {
        // increase path
        let mut path = components.to_vec();
        while path.len() < depth {
            path.push(file_name);
        }
        adjusted_path.push_str(&path.join("/"));
    }
    adjusted_path
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

fn get_files_with_extension(extension: &str, path: &str) -> Vec<String> {
    let mut files = Vec::new();
    let walker = WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !is_hidden(e));
    for entry in walker.filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            if let Some(file_extension) = entry.path().extension() {
                if file_extension == extension {
                    files.push(String::from(
                        entry.path().to_str().unwrap().strip_prefix(path).unwrap(),
                    ));
                }
            }
        }
    }
    files
}

#[derive(Debug, Clone)]
enum Depth {
    Root,
    Part,
    Chapter,
    Scene,
    Beat,
}

impl Depth {
    fn next_level(self) -> Depth {
        match self {
            Depth::Root => Depth::Part,
            Depth::Part => Depth::Chapter,
            Depth::Chapter => Depth::Scene,
            Depth::Scene => Depth::Beat,
            Depth::Beat => Depth::Beat,
        }
    }
    fn from_string(depth: &str) -> Depth {
        match depth {
            "part" => Depth::Root,
            "chapter" => Depth::Part,
            "scene" => Depth::Chapter,
            "beat" => Depth::Scene,
            _ => Depth::Beat,
        }
    }
    fn to_usize(&self) -> usize {
        match self {
            Depth::Root => 4,
            Depth::Part => 3,
            Depth::Chapter => 2,
            Depth::Scene => 1,
            Depth::Beat => 0,
        }
    }
}

#[derive(Debug)]
pub struct ContentIndex {
    depth: Depth,
    name: String,
    context: Vec<String>,
    children: Vec<ContentIndex>,
}
impl ContentIndex {
    pub fn write_latex(&self) -> String {
        let mut latex_string = String::new();
        self.print(&mut latex_string);
        latex_string
    }
    fn print(&self, page: &mut String) {
        match self.depth {
            Depth::Root => self.print_root(page),
            Depth::Part => self.print_part(page),
            Depth::Chapter => self.print_chapter(page),
            Depth::Scene => self.print_scene(page),
            Depth::Beat => self.print_beat(page),
        }
    }
    fn print_root(&self, page: &mut String) {
        for part in &self.children {
            part.print(page);
        }
    }
    fn print_part(&self, page: &mut String) {
        if self.name != "#ROOT#" {
            page.push_str(&format!("\\part{{{}}}\n", self.name));
        }
        for chapter in &self.children {
            chapter.print(page);
        }
    }
    fn print_chapter(&self, page: &mut String) {
        if self.name != "#ROOT#" {
            page.push_str(&format!("\\chapter{{{}}}\n", self.name));
        }
        let mut scenes = self.children.iter();
        if let Some(first_scene) = scenes.next() {
            first_scene.print(page);
            for scene in scenes {
                page.push_str("***\n");
                scene.print(page);
            }
        }
    }
    fn print_scene(&self, page: &mut String) {
        for beat in &self.children {
            beat.print(page);
        }
    }
    fn print_beat(&self, page: &mut String) {
        let mut adjusted_path = String::new();
        let path = &self.context[1..];
        adjusted_path.push_str(&path.join("/"));
        if path.len() > 0 {
            adjusted_path.push('/');
        }
        adjusted_path.push_str(&self.name);
        page.push_str(&format!("\\input{{content/{}}}\n", adjusted_path));
    }
}

fn add_to_tree(tree: &mut ContentIndex, parts: &[&str], depth: Depth) {
    if parts.is_empty() {
        return;
    }
    let child_depth = depth.next_level();
    let mut found = false;
    for child in &mut tree.children {
        if child.name == parts[0] {
            add_to_tree(child, &parts[1..], child_depth.clone());
            found = true;
            break;
        }
    }
    if !found {
        let mut context = tree.context.clone();
        context.push(tree.name.clone());
        let mut new_node = ContentIndex {
            name: parts[0].to_string(),
            children: Vec::new(),
            context,
            depth: child_depth.clone(),
        };
        add_to_tree(&mut new_node, &parts[1..], child_depth.clone());
        tree.children.push(new_node);
    }
}

fn create_tree(paths: Vec<String>, depth: Depth) -> ContentIndex {
    let mut root = ContentIndex {
        name: "#ROOT#".to_string(),
        children: Vec::new(),
        context: Vec::new(),
        depth: depth.clone(),
    };
    for path in paths {
        let parts: Vec<&str> = path.split('/').collect();
        add_to_tree(&mut root, &parts, depth.clone());
    }
    root
}

pub fn get_content(
    dir: &str,
    extension: &str,
    depth_string: &str,
) -> (ContentIndex, Vec<ContentFile>) {
    let depth = Depth::from_string(depth_string);
    let files_with_extension = get_files_with_extension(extension, dir);

    let content: Vec<ContentFile> = files_with_extension
        .iter()
        .map(|file| {
            let corrected_path =
                adjust_path_depth(file, depth.to_usize(), &format!(".{}", extension));
            ContentFile {
                origin: file.to_string(),
                destination: corrected_path,
            }
        })
        .collect();

    let paths: Vec<String> = content
        .iter()
        .map(|file| file.destination.clone())
        .collect();

    (create_tree(paths, depth), content)
}

//fn main() {
//  let extension = "txt";  // Change this to your desired extension
//  let dir = "content/"; //MUST ABSOLUTELY HAVE THE "/" IN THE END
//  let depth_string="part";
//  let (tree,content) = get_content(dir,extension,depth_string);
//  let page = tree.write_latex();
//  println!("{}",page);
//  for content_file in content.iter(){
//    println!("{}{} -> temp_dir/content/{}",dir,content_file.origin, content_file.destination);
//  }
//}
