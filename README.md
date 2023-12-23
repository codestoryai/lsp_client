# lsp_client

### How to use
- You can follow the example in the main.rs on how to use the library, the benefit of this api is not having to deal with the json rpc protocol, you can just call the methods and get the results.

### Why did we build this?
- LSP have a special json rpc protocol, its not straightforward to use it and you probably want to make sure that the requests are handled correctly, so we are share our implementation in the hopes that others can use it for fun and profit.