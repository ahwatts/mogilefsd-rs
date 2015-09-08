use mogilefs_common::{MogError, MogResult, Request};
use mogilefs_common::requests::*;
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Write};
use std::sync::{Arc, RwLock};
use super::super::backend::{StorageBackend, StorageMetadata, TrackerBackend, TrackerMetadata};
use super::{Domain, FileInfo};
use time;
use url::Url;

#[derive(Debug)]
pub struct MemBackend {
    domains: HashMap<String, Domain>,
    empty_domain: Domain,
    pub base_url: Url,
}

impl MemBackend {
    pub fn new(storage_base_url: Url) -> MemBackend {
        MemBackend {
            domains: HashMap::new(),
            empty_domain: Domain::new(""),
            base_url: storage_base_url,
        }
    }

    // Tracker methods.

    pub fn create_domain(&mut self, req: &CreateDomain) -> MogResult<<CreateDomain as Request>::ResponseType> {
        if self.domains.contains_key(&req.domain) {
            Err(MogError::DomainExists(req.domain.clone()))
        } else {
            let domain = Domain::new(&req.domain);
            self.domains.insert(req.domain.clone(), domain);
            Ok(())
        }
    }

    pub fn create_open(&mut self, req: &CreateOpen) -> MogResult<<CreateOpen as Request>::ResponseType> {
        let fid = self.domains.len() + 1;
        let url = self.url_for_key(&req.domain, &req.key);
        let domain = try!(self.domain_mut(&req.domain));
        let file_info = FileInfo::new(&req.key);
        try!(domain.add_file(&req.key, file_info));

        let mut response = CreateOpenResponse {
            fid: fid as u64,
            devcount: 1,
            paths: HashMap::new(),
        };
        response.paths.insert(1, url);
        Ok(response)
    }

    pub fn create_close(&mut self, _domain: &str, _key: &str, _path: &Url, _size: u64) -> MogResult<()> {
        // There's really nothing to do here; we presumably could
        // verify that the file was uploaded to the URL, but ehh.
        Ok(())
    }

    pub fn file_info(&self, domain: &str, key: &str) -> MogResult<TrackerMetadata> {
        self.domain(domain)
            .and_then(|d| d.file(key).ok_or(MogError::UnknownKey(key.to_string())))
            .map(|file_info| {
                TrackerMetadata {
                    size: file_info.size.unwrap_or(0),
                    domain: domain.to_string(),
                    key: key.to_string(),
                }
            })
    }

    pub fn get_paths(&self, domain: &str, key: &str) -> MogResult<Vec<Url>> {
        self.domain(domain)
            .and_then(|d| d.file(key).ok_or(MogError::UnknownKey(key.to_string())))
            .map(|_| vec![ self.url_for_key(domain, key) ])
    }

    pub fn delete(&mut self, domain: &str, key: &str) -> MogResult<()> {
        try!(self.domain_mut(domain))
            .remove_file(key)
            .map(|_| ())
            .ok_or(MogError::UnknownKey(key.to_string()))
    }

    pub fn rename(&mut self, domain: &str, from: &str, to: &str) -> MogResult<()> {
        self.domain_mut(domain).and_then(|d| d.rename(from, to))
    }

    pub fn list_keys(&self, domain_name: &str, prefix: Option<&str>, after_key: Option<&str>, limit: Option<usize>) -> MogResult<Vec<String>> {
        let after_key = after_key.unwrap_or("");
        let prefix = prefix.unwrap_or("");
        let limit = limit.unwrap_or(1000);
        Ok(try!(self.domain(domain_name)).files()
           .skip_while(|&(k, _)| k <= after_key || !k.starts_with(prefix))
           .take(limit)
           .map(|(k, _)| k.to_string())
           .collect())
    }

    // Storage server methods.

    pub fn url_for_key(&self, domain: &str, key: &str) -> Url {
        url_for_key(&self.base_url, domain, key)
    }

    pub fn file_metadata(&self, domain: &str, key: &str) -> MogResult<StorageMetadata> {
        let file_info = try!(try!(self.file(domain, key)).ok_or(MogError::UnknownKey(key.to_string())));

        match (file_info.size, file_info.mtime) {
            (Some(size), Some(mtime)) => {
                Ok(StorageMetadata { size: size, mtime: mtime, })
            },
            _ => {
                Err(MogError::NoContent(key.to_string()))
            }
        }
    }

    pub fn store_reader_content<R: Read>(&mut self, domain: &str, key: &str, reader: &mut R) -> MogResult<()> {
        let mut content = vec![];
        try!(io::copy(reader, &mut content));
        self.store_bytes_content(domain, key, &content)
    }

    pub fn store_bytes_content(&mut self, domain: &str, key: &str, content: &[u8]) -> MogResult<()> {
        let file_info = try!(try!(self.file_mut(domain, key)).ok_or(MogError::UnknownKey(key.to_string())));
        file_info.size = Some(content.len() as u64);
        file_info.content = Some(content.to_owned());
        file_info.mtime = Some(time::now_utc());
        Ok(())
    }

    pub fn get_content<W: Write>(&self, domain: &str, key: &str, writer: &mut W) -> MogResult<()> {
        let file_info = try!(try!(self.file(domain, key)).ok_or(MogError::UnknownKey(key.to_string())));
        match file_info.content {
            Some(ref reader) => {
                try!(io::copy(&mut Cursor::new(reader.as_ref()), writer));
                Ok(())
            },
            None => {
                Err(MogError::NoContent(key.to_string()))
            }
        }
    }

    // Utility methods.

    fn file(&self, domain: &str, key: &str) -> MogResult<Option<&FileInfo>> {
        self.domain(domain).map(|d| d.file(key))
    }

    fn file_mut(&mut self, domain: &str, key: &str) -> MogResult<Option<&mut FileInfo>> {
        self.domain_mut(domain).map(|d| d.file_mut(key))
    }

    fn domain(&self, domain_name: &str) -> MogResult<&Domain> {
        // self.domains.get(domain_name).ok_or(MogError::UnregDomain(domain_name.to_string()))
        Ok(self.domains.get(domain_name).unwrap_or(&self.empty_domain))
    }

    fn domain_mut(&mut self, domain_name: &str) -> MogResult<&mut Domain> {
        // self.domains.get_mut(domain_name).ok_or(MogError::UnregDomain(domain_name.to_string()))
        Ok(self.domains.entry(domain_name.to_string()).or_insert(Domain::new(domain_name)))
    }
}

#[derive(Clone, Debug)]
pub struct SyncMemBackend(Arc<RwLock<MemBackend>>, Url);

impl SyncMemBackend {
    pub fn new(backend: MemBackend) -> SyncMemBackend {
        let base_url = backend.base_url.clone();
        SyncMemBackend(Arc::new(RwLock::new(backend)), base_url)
    }

    pub fn with_file<F>(&self, domain: &str, key: &str, block: F) -> MogResult<()>
        where F: FnOnce(&FileInfo) -> MogResult<()>
    {
        let guard = try!(self.0.read());
        match guard.file(domain, key) {
            Ok(Some(ref file_info)) => block(file_info),
            Ok(None) => Err(MogError::UnknownKey(key.to_string())),
            Err(e) => Err(e),
        }
    }

    pub fn with_file_mut<F>(&self, domain: &str, key: &str, block: F) -> MogResult<()>
        where F: FnOnce(&mut FileInfo) -> MogResult<()>
    {
        let mut guard = try!(self.0.write());
        match guard.file_mut(domain, key) {
            Ok(Some(ref mut file_info)) => block(file_info),
            Ok(None) => Err(MogError::UnknownKey(key.to_string())),
            Err(e) => Err(e),
        }
    }
}

impl TrackerBackend for SyncMemBackend {
    fn create_domain(&self, request: &CreateDomain) -> MogResult<<CreateDomain as Request>::ResponseType> {
        try!(self.0.write()).create_domain(&request)
    }

    fn create_open(&self, request: &CreateOpen) -> MogResult<<CreateOpen as Request>::ResponseType> {
        try!(self.0.write()).create_open(&request)
    }

    fn create_close(&self, _domain: &str, _key: &str, _url: &Url, _size: u64) -> MogResult<()> {
        // There's nothing to do here. See the equivalent method on
        // the actual backend. There's no need acquire the mutex and
        // call it, since we're not going to be doing anything with
        // it anyway.
        Ok(())
    }

    fn file_info(&self, domain: &str, key: &str) -> MogResult<TrackerMetadata> {
        try!(self.0.read()).file_info(domain, key)
    }

    fn get_paths(&self, domain: &str, key: &str) -> MogResult<Vec<Url>> {
        try!(self.0.read()).get_paths(domain, key)
    }

    fn delete(&self, domain: &str, key: &str) -> MogResult<()> {
        try!(self.0.write()).delete(domain, key)
    }

    fn rename(&self, domain: &str, from: &str, to: &str) -> MogResult<()> {
        try!(self.0.write()).rename(domain, from, to)
    }

    fn list_keys(&self, domain: &str, prefix: Option<&str>, after_key: Option<&str>, limit: Option<usize>) -> MogResult<Vec<String>> {
        try!(self.0.read()).list_keys(domain, prefix, after_key, limit)
    }
}

impl StorageBackend for SyncMemBackend {
    fn url_for_key(&self, domain: &str, key: &str) -> Url {
        url_for_key(&self.1, domain, key)
    }

    fn file_metadata(&self, domain: &str, key: &str) -> MogResult<StorageMetadata> {
        try!(self.0.read()).file_metadata(domain, key)
    }

    fn store_reader_content<R: Read>(&self, domain: &str, key: &str, reader: &mut R) -> MogResult<()> {
        try!(self.0.write()).store_reader_content(domain, key, reader)
    }

    fn store_bytes_content(&self, domain: &str, key: &str, content: &[u8]) -> MogResult<()> {
        try!(self.0.write()).store_bytes_content(domain, key, content)
    }

    fn get_content<W: Write>(&self, domain: &str, key: &str, writer: &mut W) -> MogResult<()> {
        try!(self.0.read()).get_content(domain, key, writer)
    }
}

pub fn url_for_key(base_url: &Url, domain: &str, key: &str) -> Url {
    let mut key_url = base_url.clone();
    let mut new_path = Vec::from(key_url.path().unwrap());
    new_path.extend([ "d", domain, "k" ].iter().map(|s| s.to_string()));
    new_path.extend(key.split("/").map(|s| s.to_string()));
    new_path = new_path.into_iter().skip_while(|p| p == "").collect();
    *key_url.path_mut().unwrap() = new_path;
    key_url
}

#[cfg(test)]
mod tests {
    use mogilefs_common::MogError;
    use mogilefs_common::requests::*;
    use std::io::Cursor;
    use super::super::super::backend::TrackerBackend;
    use super::super::super::test_support::*;

    #[test]
    fn backend_get_file() {
        let mut backend = backend_fixture();

        {
            let file = backend.file(TEST_DOMAIN, TEST_KEY_1);
            assert!(
                matches!(file, Ok(Some(ref f)) if f.key() == TEST_KEY_1),
                "Immutable present file was {:?}", file);
        }

        {
            let file = backend.file(TEST_DOMAIN, "test/key/3");
            assert!(
                matches!(file, Ok(None)),
                "Immutable missing file was {:?}", file);
        }

        {
            let file = backend.file_mut(TEST_DOMAIN, TEST_KEY_1);
            assert!(
                matches!(file, Ok(Some(ref f)) if f.key() == TEST_KEY_1),
                "Mutable present file was {:?}", file);
        }

        {
            let file = backend.file_mut(TEST_DOMAIN, "test/key/3");
            assert!(
                matches!(file, Ok(None)),
                "Mutable missing file was {:?}", file);
        }

        // {
        //     let file = backend.file("test_domain_2", TEST_KEY_1);
        //     assert!(
        //         matches!(file, Err(MogError::UnregDomain(ref d)) if d == "test_domain_2"),
        //         "Immutable file from nonexistent domain was {:?}", file);
        // }

        // {
        //     let file = backend.file_mut("test_domain_2", TEST_KEY_1);
        //     assert!(
        //         matches!(file, Err(MogError::UnregDomain(ref d)) if d == "test_domain_2"),
        //         "Mutable file from nonexistent domain was {:?}", file);
        // }
    }

    #[test]
    fn backend_create_domain() {
        let mut backend = backend_fixture();

        let create_request = CreateDomain { domain: "test_domain_2".to_string() };
        let create_result = backend.create_domain(&create_request);
        assert!(create_result.is_ok(), "Create new domain result was {:?}", create_result);

        assert!(backend.domains.contains_key("test_domain_2"));

        let create_dup_request = CreateDomain { domain: TEST_DOMAIN.to_string() };
        let create_dup_result = backend.create_domain(&create_dup_request);
        assert!(
            matches!(create_dup_result, Err(MogError::DomainExists(ref d)) if d == TEST_DOMAIN),
            "Create duplicate domain result was {:?}", create_dup_result);
    }

    #[test]
    fn backend_create_open() {
        use url::Url;

        let sync_backend = sync_backend_fixture();
        // let storage = MemStorage::new(
        //     sync_backend.clone(),
        //     Url::parse(format!("http://{}/{}", TEST_HOST, TEST_BASE_PATH).as_ref()).unwrap());

        {
            let req = CreateOpen { domain: TEST_DOMAIN.to_string(), key: "test/key/3".to_string(), multi_dest: true, size: None };
            let mut backend = sync_backend.0.write().unwrap();
            let co_result = backend.create_open(&req);
            assert!(co_result.is_ok());
            let co_response = co_result.unwrap();
            assert_eq!(1, co_response.paths.len());
            assert_eq!(
                &Url::parse(format!("http://{}/{}/d/{}/k/{}", TEST_HOST, TEST_BASE_PATH, TEST_DOMAIN, "test/key/3").as_ref()).unwrap(),
                co_response.paths.iter().next().unwrap().1);
        }

        {
            let backend = sync_backend.0.read().unwrap();
            let file = backend.file(TEST_DOMAIN, "test/key/3");
            assert!(matches!(file, Ok(Some(..))), "Create opened file was {:?}", file);
            let file = file.unwrap().unwrap();
            assert_eq!("test/key/3", file.key());
            assert!(file.content.is_none());
            assert!(file.size.is_none());
        }

        {
            let req = CreateOpen { domain: TEST_DOMAIN.to_string(), key: TEST_KEY_1.to_string(), multi_dest: true, size: None };
            let mut backend = sync_backend.0.write().unwrap();
            let co_result = backend.create_open(&req);
            assert!(co_result.is_ok(), "Create open with duplicate key result was {:?}", co_result);
            let co_response = co_result.unwrap();
            assert_eq!(1, co_response.paths.len());
            assert_eq!(
                &Url::parse(format!("http://{}/{}/d/{}/k/{}", TEST_HOST, TEST_BASE_PATH, TEST_DOMAIN, TEST_KEY_1).as_ref()).unwrap(),
                co_response.paths.iter().next().unwrap().1);
        }

        // {
        //     let mut backend = sync_backend.0.lock().unwrap();
        //     let co_result = backend.create_open("test_domain_2", "test/key/3", &storage);
        //     assert!(
        //         matches!(co_result, Err(MogError::UnregDomain(ref k)) if k == "test_domain_2"),
        //         "Create open with unknown domain result was {:?}", co_result);
        // }
    }

    #[test]
    fn domain_list_keys() {
        let backend = backend_fixture();
        let list_result = backend.list_keys(TEST_DOMAIN, None, None, None);
        assert!(list_result.is_ok());
        assert_eq!(vec![ TEST_KEY_1, TEST_KEY_2 ], list_result.unwrap());
    }

    #[test]
    fn domain_list_keys_limit() {
        let backend = full_backend_fixture();
        let list_result = backend.list_keys(TEST_FULL_DOMAIN, None, None, Some(10));
        assert!(list_result.is_ok());
        let list = list_result.unwrap();
        assert_eq!(10, list.len());
        assert!(list[0] < list[9]);
    }

    #[test]
    fn domain_list_keys_after() {
        let backend = full_backend_fixture();
        let first_list = backend.list_keys(TEST_FULL_DOMAIN, None, None, Some(10)).unwrap();
        let after_key = first_list.iter().last().unwrap();

        let list_result = backend.list_keys(TEST_FULL_DOMAIN, None, Some(after_key), None);
        assert!(list_result.is_ok());
        let list = list_result.unwrap();
        assert!(after_key < &list[0]);
        assert!(&list[0] < list.iter().last().unwrap());
    }

    #[test]
    fn domain_list_keys_prefix() {
        let backend = full_backend_fixture();
        let list_result = backend.list_keys(TEST_FULL_DOMAIN, Some(TEST_KEY_PREFIX_1), None, None);
        assert!(list_result.is_ok());
        let list = list_result.unwrap();
        for key in list.iter() {
            assert!(key.starts_with(TEST_KEY_PREFIX_1), "key {:?} doesn't start with {:?}", key, TEST_KEY_PREFIX_1);
        }
    }

    #[test]
    fn domain_delete_key() {
        let mut backend = backend_fixture();

        {
            let delete_result = backend.delete(TEST_DOMAIN, TEST_KEY_1);
            assert!(matches!(delete_result, Ok(())));
        }

        assert!(backend.domains[TEST_DOMAIN].file(TEST_KEY_1).is_none());

        {
            let delete_result_2 = backend.delete(TEST_DOMAIN, TEST_KEY_1);
            assert!(matches!(delete_result_2, Err(MogError::UnknownKey(ref k)) if k == TEST_KEY_1))
        }
    }

    #[test]
    fn url_for_key() {
        let backend = backend_fixture();
        assert_eq!(
            format!("http://{}/{}/d/{}/k/{}", TEST_HOST, TEST_BASE_PATH, TEST_DOMAIN, TEST_KEY_1),
            backend.url_for_key(TEST_DOMAIN, TEST_KEY_1).serialize());
    }

    #[test]
    fn get_content() {
        let backend = backend_fixture();
        let mut content = vec![];

        backend.get_content(TEST_DOMAIN, TEST_KEY_1, &mut content).unwrap_or_else(|e| {
            panic!("Error retrieving content from {:?}: {}", TEST_KEY_1, e);
        });

        let content_ref: &[u8] = &content;
        assert_eq!(TEST_CONTENT_1, content_ref);
    }

    #[test]
    fn get_content_unknown_key() {
        let backend = backend_fixture();
        let mut content = vec![];
        assert!(matches!(backend.get_content(TEST_DOMAIN, "test/key/3", &mut content).unwrap_err(),
                         MogError::UnknownKey(ref k) if k == "test/key/3"));
        assert!(content.is_empty());
    }

    #[test]
    fn get_content_no_content() {
        let backend = backend_fixture();
        let mut content = vec![];
        assert!(matches!(backend.get_content(TEST_DOMAIN, TEST_KEY_2, &mut content).unwrap_err(),
                         MogError::NoContent(ref k) if k == TEST_KEY_2));
        assert!(content.is_empty());
    }

    #[test]
    fn store_replace_content() {
        let mut backend = backend_fixture();
        let new_content = Vec::from("This is new test content");

        backend.store_reader_content(TEST_DOMAIN, TEST_KEY_1, &mut Cursor::new(new_content.clone())).unwrap_or_else(|e| {
            panic!("Error storing content to {:?}: {}", TEST_KEY_1, e);
        });

        assert_eq!(&new_content, backend.domains[TEST_DOMAIN].file(TEST_KEY_1).unwrap().content.as_ref().unwrap());
    }

    #[test]
    fn store_new_content() {
        let mut backend = backend_fixture();
        let new_content = Vec::from("This is new test content");

        backend.store_reader_content(TEST_DOMAIN, TEST_KEY_2, &mut Cursor::new(new_content.clone())).unwrap_or_else(|e| {
            panic!("Error storing content to {:?}: {}", TEST_KEY_2, e);
        });

        assert_eq!(&new_content, backend.domains[TEST_DOMAIN].file(TEST_KEY_2).unwrap().content.as_ref().unwrap());
    }

    #[test]
    fn store_content_to_unknown_key() {
        let mut backend = backend_fixture();
        let new_content: &'static [u8] = b"This is new test content";
        assert!(matches!(backend.store_reader_content(TEST_DOMAIN, "test/key/3", &mut Cursor::new(new_content)).unwrap_err(),
                         MogError::UnknownKey(ref k) if k == "test/key/3"));
    }
}

#[cfg(test)]
pub mod test_support {
    use std::collections::HashMap;
    use super::*;
    use super::super::Domain;
    use super::super::model::test_support::{domain_fixture, full_domain_fixture};
    use url::Url;

    pub static TEST_HOST: &'static str = "test.host";
    pub static TEST_BASE_PATH: &'static str = "base_path";

    lazy_static!{
        static ref TEST_BASE_URL: Url = Url::parse(&format!("http://{}/{}", TEST_HOST, TEST_BASE_PATH)).unwrap();
    }

    pub fn backend_fixture() -> MemBackend {
        let mut backend = MemBackend {
            domains: HashMap::new(),
            empty_domain: Domain::new(""),
            base_url: TEST_BASE_URL.clone(),
        };
        let domain = domain_fixture();
        backend.domains.insert(domain.name().to_string(), domain);
        backend
    }

    pub fn full_backend_fixture() -> MemBackend {
        let mut backend = MemBackend {
            domains: HashMap::new(),
            empty_domain: Domain::new(""),
            base_url: TEST_BASE_URL.clone(),
        };
        let domain = full_domain_fixture();
        backend.domains.insert(domain.name().to_string(), domain);
        backend
    }

    pub fn sync_backend_fixture() -> SyncMemBackend {
        SyncMemBackend::new(backend_fixture())
    }
}