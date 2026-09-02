built in rust, and exposes a rust library for usage.

space station is an observability tool that accepts records for a table within an organization. and then a carbon/silicon can create space-windows depending on the information being looked for.

Org > Tables > Records

i'll write pseudocode (in python style), to be implemented in rust eventually:
event flow:
```python
from space_station import SpaceClient

ss = SpaceClient(table_key="...") # table key is associated with a table + org

@omni.on_event
def record_event(event):
    ss.record({...}) # non blocking
```
there can be a rust space station deamon running in the background that is always running. it maintains a ws connection with the space station server to send data. there could be a lot of organizations running on the same system, so it saves a LOT of network requests. there is no batching locally. send data as soon as possible. & store data in disk so in case of an error / crash, it can be restored & sent when system is back up. the server sends an ACK with the batchid it just registered. if the batch fails to upload, then it is sent again + any new records that might have been registered in the meanwhile. max size of a batch is 8MB.
in case of rejection: { batch_id, status: "rejected", rejected: [{"record_id": "reason (duplicate, size exeeded, etc etc, each should have a code that can be used to decide)"}] }

the rust library automatically adds the following metadata:
record_id (uuid), table id, system specs (cached), cpu usage, gpu usage, ram usage, storage left, record timestamp in ms,

then it finally sends something like
{
    "metadata": {...},
    "record": {...}
}

for now, the rust library will only accepts records (max size per value is 32kb, max size for record is 256kb. replace any file (Data URI prefix, Magic bytes, Base64 shape, Non-UTF8 or control-char-heavy) value with "[FILETYPE:SIZE]", truncate any big strings at the middle to be max os 32kb. truncate like "abc...[SIZE]...xyz" with size before truncation included).

when creating tables & space windows, it can be given access via iam access to @carbon, @silicon or tags. the union is considered for access.

backend:
the backend batches and inserts only when either elapsed ≥ 1s || bytes ≥ 16MB. this is global batching of all events and added together once using one insert.
uses ClickHouse on AWS to store the records inside their respective tables (named "tableid:orgid").
+ connected with a rust based backend to handle library ingest + frontend queries & handle authentication with iam + subscription pub/sub.
once the backend has received and registered the record, record registered timestamp is added to the metadata which is also in miliseconds & adds a cursor (UInt64) to this record based on the table + org (starts at 1). the cursor is assigned once after the batch is registered and the records arrive. record register time in ms is also added at this point. this is only point when a cusor is decided and no where else. queries only run on records that have already been added to clickhouse.
in case of boot / failover, this cursor assignment owner reads the max(cursor) in clickhouse and shifts forward by +10000
backend also filters org queries and run it via active org tab subcriptions to publish to the clients who have subscribed with a possibility to ask a given cursor.
there is a local redis with disk save to store all records before it has been added to clickhouse. it is removed only after the record has been successfully added to clickhouse.
store thing inside tables chronologically: ORDER BY (event_ts_ms)

Deduplication:
a list of record ids are stored on redis with a TTL of 5min where it is checked if the record id is the same and rejects any event that has already been added in the last 5min with this record id. because record ids are uuid, it should be almost no id collisions for non duplicate requests in 5mins.

Watermark is the highest known/queried cursor in a given table. Useful during filter as the last known cursor may not be the last queried cursor. it is usually the highest cursor in the table.

frontend:
login via iam.
on the left hand sidebar, they see all the orgs they are part of.
click into any of the orgs.
there are 3 tabs (Space Windows, Tables, Notifications, Settings).
if no table exists, then table view is the default, else space window.

table view:
Overview:
total number of tables created, total number of records, list of 5 of the most active tables in the last [1m, 5m, 15m, 1h, 5h (default), 1d, 7d, 30d] with a live counter of how many records in each in the last X timeframe, avg duration between event timestamp vs recorded timestamp over the last 100 records.

then a list of all the tables created and the number of records each on has + make new table button.

if nothing exists, then it just shows a big button to create the first table. it only asks for table id which is anything unique inside the org, lowercase alphanumeric. when its generated, a key is shown one time only, it is then hashed and stored. there can only be one key at a time for a table, and it can be rotated.

Space Windows:
If no space window has been created, then it says "Create your first Space Window"

space window is Processor -> Renderer.
This is how carbons see their data. Silicons use Processor's output as feed.

Processor & Renderer
Processor is responsible for handling records, processing it and finally returning a SiliconJSON(minimal JSON object) for renderer to load views. This JSON can be max 64KB. Processor also exposes tools that can be called.

```js
// SiliconJSON: {...} -> as close to final render as possible
// ProcessorData {table: {...}, ...} -> stores last_cursor, watermark, etc etc. managed on the client side.

export default defineProcessor({
  init: (silicon_json) => {
    // mission_control.query("SQL")
    return silicon_json // replaces SiliconJSON
  }
  subscriptions: {
    sub1: {
      triggers: [{ table: "...", where: "..." }],
      sql:  "...",
      mode: "delta",
      onTrigger: (silicon_json, new_rows) => {... return silicon_json}, // replaces SiliconJSON
    },
    sub2: {
      triggers: [{ table: "...", where: "..." }],
      sql:  "...",
      mode: "snapshot",
      onTrigger: (silicon_json, new_rows) => {... return silicon_json}, // replaces SiliconJSON
    },
  },

  tools: {
    order_detail: {
      args: { order_id: "string" },
      run: (silicon_json, { order_id }) => { return some_json }
    },
    filter_orders: {
      args: { min_amount: "number" },
      run: (silicon_json, { min_amount }) => {..., return some_json}
    }
  }
});
```

when restarting, the init code is run before even registering any subscriptions. it seeds or builds on top of SiliconJson and ProcessorData. after its done, subscriptions happen that can take responsibility of updates after init. Init basically houses the code of all the subscriptions. Its advised to make it a function and reuse the code. it then sets the cursors and watermarks for all further queries.

since multiple subscriptions edit the same SiliconJSON, subscriptions are run syncronously.
while one is running, the others wait in a queue. if two happen at exactly the same time, then the one higher in the subscriptions dict definition is favoured. if sub1 is trigger multiple times while sub2 was running... it is all combined into one run.

onTrigger can also run mission_control.query commands if needed. and each on trigger has a timeout of 10sec. if it returns an error, then dont move the last_cursor. just move to the next in the queue.

Renders is an iframe that gets the SiliconJSON + Tools + Metadata, an event when the json is updated & list of tools. it can then render that data into anything that can be rendered within an iframe. All external requests are allowed by the renderer. In the docs, write that default &b preferred is a Vue3 app with D3. Both via CDN.

metadata: {processor_version, renderer_version, produced_at, is_live}
tools: {tool1: {min_amount: "number"}, ...}

```html
<div id="app">
  <h1>{{ mission_control.json.orders[0]?.name }}</h1>
  <p v-if="stale">as of {{ mission_control.metadata.produced_at }}</p>
  <ul><li v-for="o in mission_control.json.recent" :key="o.id">{{ o.customer }} — ${{ o.amount }}</li></ul>
</div>
<script src="https://cdnjs.cloudflare.com/ajax/libs/vue/3.5.13/vue.global.prod.min.js"></script>
<script>
    mission_control.mount("#app");

    // should be possible
    // await mission_control.tools.filter_orders({ min_amount: 5 }); also does type checking. optional is also possible.
</script>
```

renderer can not directly do .query and .subscribe on mission control.

the SQL written can access any table inside this org via its tableid. and it can only perform read-only queries on the table. on the backend, the SQL is converted to AST, FROM is only allowed to be a tableid that is owned by the org. url(), remote(), etc SOURCES commands are strictly not allowed. readonly=1, allow_ddl=0, and it should have no access to system logs.

the user can choose the name of the space window (under 20 characters) which will be displayed as the title. once created, it shows a prompt that can be given to any agent that will write the code for this space window. and a button to add code.

space windows are version managed & version named.

Docs:
For creating Space Windows, create a npm package that can mimic the prod environment using the access token stored inside .env

After that, make the development env identical to the prod as in the code can be pasted here and run exactly the same. It should be possible to trigger notifications on last knows trigger or say there was none.

make sure the processor & renderer boundary is also implemented well in the dev environment and works identical to prod. the code should not require pasting the access token. only .env file should have it.



since the record is very minimal and stores any json they pass in as record, its really upto the org to send data they care about, plan how they wanna use it, and then create views around it.

when in the space window view, up top, they can also see a space-window access-token & last used time and date. they can use this access token during development. this will allow mission control to send data without being in the space window using their access token. this access token is tied to the carbon + org. there can only be one live and be rotated easily.

we'll need to make a space station cdn package that can handle both prod and dev. during dev it takes in the access token. dev code is not allowed to be uploaded. if it is prod ready, then it asks for a name for this version and uploads it.

to check for prod readiness, when the space-window's code is pasted, it checks for `spacewindow-[32digitshexadecimal]` which is always the shape of the access token... and rejects that code from being used. this helps detect access token in comments which above method would fail to catch.

the iframe doesnt get to access any cookie from the parent. but it can render & call any cdn, or third party services it needs to show the view.

Active Subscriptions:
Only when someone is active on a space window with a subscription, does the backend run the query and return it. it is client's responsibility to manage cursors in case of disconnected ws request.

Notifications:
These are similar to active subscritions but are of always active & returns a notification.
```json
{
  name:         "New order from recent signup",
  description:  "...",                    // optional, for the team
  enabled:      true,

  triggers:     [{ table: "orders", where: "price > 5" }],
                // or [{ schedule: "*/5 * * * *" }] for absence detection or scheduled summaries
                // array

  sql:          "...",                    // must return {dedup_key, text, metadata}

  delay:        "2s",                     // default 2s
  cooldown:     "10m",                    // default 10m
  access:       ["@...", "tags"],         // who can access this notification
  recipients:   ["@...", "@...", "webhook:webhook-id"],           // on a subscribe basis, a subset of access.
}
```
triggers are list of triggers based on a table, or cron.
sql must ruturn the type {dedup_key: ..., text: "...", metadata: {...}}
dedup key is used to not send the same notification again within the cooldown period.
delay is how long after the trigger is hit, does the sql gets run so that if the sql depends on any other table, it gets filled before running sql.

webhooks are also possible recipients of a notification,  the entire {dedup_key, text, metadata} is sent to the webhook. webhooks follow the standard practice of using the webhook secret and passing a signature.

Once a notification is added to postgres, it is counted as read. This is an append only system with no read/unread status.

Settings:
webhooks (id, url & secret)
api keys (scoped:[tables(read only)], notifications (read only))

Silicons:
Silicons can also use space station. they can do everything that a carbon can. make tables, make space windows, see (get code of) windows they have access to, get their access token that they can use to run the queries, subscribe to notifications (by giving a webhook url), when a silicon is added to recipients, it goes to the webhook attached in the silicon's profile. silicon is visible like any other carbon with a @..., just that its also sent on a webhook associated with that silicon.
silicons can also query past notifications. they can create new notifications. they can even create windows for carbons by writing the code & testing it locally, and publishing it on space station.

silicons can also use the space station cli tool to view all the data.
silicons first authenticate to space station via `silicon-iam` and then use the authtoken to make requests. first time, a silicon can run space station auth which takes its silicon id and silicon token and authenticates the silicon. silicon token should never be sent to space station, only auth token which is then verified independently by space station with iam.
space station cli run only the processors for a requested window locally and keeps a 10min idle before marking that a window is inactive.
for silicons, the Space Window is the Processor's output (SiliconJSON).
it can query and get the latest caches SiliconJSON and run tools to inspect deeper.

Dev Mode:
It should be possible to enter and do things in dev mode which will show errors inside Notificaitons, Processor or Renderer. It is possible to see the errors as both carbons (in the UI using option shift d) or as a silicon in the cli.

Credentials:
Auth Token: what allows carbons and silicons to do things they are allowed to
Access Token: carbons & silicons can write code that can call queries & notifications on their behalf via mission control to either get data locally, or to build notifications and space windows.
API Key: Programatically view tables & notifications on behalf of the organization and no carbon/silicon.


Implementation:
we'll have ClickHouse for records, and a Postgres for all other things.
the backend is in rust, the cli is in rust with node dependency for processor, the space station client is in rust that allows other apps to send data to space station tables via table key. frontend is in nextjs. it also contains the documentation.\
When creating anything, the one who created it should be added. Like Table created be carbon A, Notification v3 created by Silicon B, etc etc.

Backend: Rust
DB: ClickHouse & Postgres
Cache: Redis
CLI & Library: Rust with other dependencies. (it is used for all client side things like creating records, setting up cli, etc etc)
Frontend: NextJS
Authentication: Silicon IAM (https://backend.iam.teamofsilicons.com/docs/client/)


# codebase
structure it in modules. each part here becomes a module. nesting module is possible into submodules. define modules based on how i've seperated ideas here.
create a shared dir for shared code.
keep the code to a minimum. if it can be done in less, lets do it in less.
we are following a event/callback driven code style.
publish the libs.
follow a sync approach when its for simple tasks, event/callback driven > async for complex. async otherwise.
write test cases, mention what you're testing a test-group, and then at the end, give results.
all tools you need are installed natively and feel free to install any package.
aws cli 


# codebase thinking
- writing code is not just about implementation, maintainability & elegance matter as much.
- test and try things before you implement. try a simpler version to see how it works, what works what doesn't work. think in extremes.
- smaller code is reliable code. write less.
- writing once is not enough. its v0.0, iterate. make it smaller, faster, reliable, resilient, elegant, & largely maintainable.
- use pre installed libraries before you need to reach out for external onces. feel free to use them when you want.
- codebase is a form of art.
- use workflows well... not just for writing code, but thinking, evaluating, testing, researching, organizing, and critiquing yourself.
- run agents to get critiques on what you have done. what you have thought.
- don't implement more than this UNDERSTANDING.md asks you until its truely needed.