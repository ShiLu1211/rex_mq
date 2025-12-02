from rex4p import *
import asyncio


class MyHandler:
    def on_login(self, data):
        print("login ok")

    def on_message(self, data):
        print(data)


async def main():
    handler = MyHandler()
    config = ClientConfig("127.0.0.1:8881", Protocol.tcp(), "one", handler)

    client = RexClient()
    await client.connect(config)

    while not await client.is_connected():
        await asyncio.sleep(0.1)

    await client.send_text(RexCommand.Title, "Hello World", target=None, title="two")

    await asyncio.sleep(10)

    await client.close()


if __name__ == "__main__":
    asyncio.run(main())
