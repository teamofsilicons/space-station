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
there can be a rust space station deamon running in the background that is always running. it maintains a ws connection with the space station server to send data. there could be a lot of organizations running on the same system, so it saves a LOT of network requests. there is no batching locally. send data as soon as possible. & store data in disk so in case of an error / crash, it can be restored & sent when system is back up. the server sends an ACK with the record_id it just registered. if something that should have been registered but failed gets in there, then it is sent again.

the rust library automatically adds the following metadata:
record_id, time, table id, system specs (cached), cpu usage, gpu usage, ram usage, storage left, record timestamp in ms,

then it finally sends something like
{
    "metadata": {...},
    "record": {...}
}

for now, the rust library will only accepts records (max size per value is 32kb, max size for record is 256kb. replace any file (Data URI prefix, Magic bytes, Base64 shape, Non-UTF8 or control-char-heavy) value with "[FILETYPE:SIZE]", truncate any big strings at the middle to be max os 32kb. truncate like "abc...[SIZE]...xyz" with size before truncation included).

only org admins + org owner can access the records.

backend:
the backend batches and inserts only when either elapsed ≥ 1s || bytes ≥ 16MB.
uses ClickHouse on AWS to store the records inside their respective tables (named "tableid:orgid").
+ connected with a rust based backend to handle library ingest + frontend queries & handle authentication with iam + subscription pub/sub.
once the backend has received and registered the record, record registered timestamp is added to the metadata which is also in miliseconds & adds a cursor number to this record based on the table (+1 record).
backend also filters org queries and run it via active org tab subcriptions to publish to the clients who have subscribed with a possibility to ask a given cursor.

frontend:
login via iam.
on the left hand sidebar, they see all the orgs they are an owner or admin of.
click into any of the orgs.
there are 3 tabs (Space Windows, Tables, Notifications).
if no table exists, then table view is the default, else space window.

table view:
Overview:
total number of tables created, total number of records, list of 5 of the most active tables in the last [1m, 5m, 15m, 1h, 5h (default), 1d, 7d, 30d] with a live counter of how many records in each in the last X timeframe, avg duration between event timestamp vs recorded timestamp over the last 100 records.

then a list of all the tables created and the number of records each on has + make new table button.

if nothing exists, then it just shows a big button to create the first table. it only asks for table id which is anything unique inside the org, lowercase alphanumeric. when its generated, a key is shown one time only, it is then hashed and stored. there can only be one key at a time for a table, and it can be rotated.

Space Windows:
If no space window has been created, then it says "Create your first Space Window"

space window is an iframe that is loaded & given a query tool that the main app executes. it uses d3js to render things by default but the iframe can load any html inside that window.
space-window has access to a tool called mission_control(...) which queries a given ClickHouse SQL & renders the view inside the iframe. the iframe doesn't ever leak the authtoken of the logged in user but mission_control is used as a proxy. the iframe needs enough sandboxing so nothing from the parent is revealed.

the SQL written can access any table inside this org via its tableid. and it can only perform read-only queries on the table. on the backend, the SQL is converted to AST, FROM is only allowed to be a tableid that is owned by the org. url(), remote(), etc SOURCES commands are strictly not allowed. readonly=1, allow_ddl=0, and it should have no access to system logs.

the user can choose the name of the space window (under 20 characters) which will be displayed as the title. once created, it shows a prompt that can be given to any agent that will write the code for this space window. and a button to add code.

space windows are version managed & version named.

mission control docs:
this is the docs that can be sent to any coding agent to write the code for the space-window. mission control is incredibly powerful and lets people query very powerful and complex queries. these are read only queries. no update, add, or delete is possible. people can make as simple or complex tabs as they like depending on what they need, and since its all just an iframe, they can do interactive and progressive things as well.

mission control also has a subscribe feature. where space-window can subscribe to a certain query for if any new data comes in and uses cursor for any missed data. 

since the record is very minimal and stores any json they pass in as record, its really upto the org to send data they care about, plan how they wanna use it, and then create views around it.

when in the space window view, up top, they can also see a space-window access-token & last used time and date. they can use this access token during development. this will allow mission control to send data without being in the space window using their access token. this access token is tied to the carbon + org. there can only be one live and be rotated easily.

we'll need to make a space station cdn package that can handle both prod and dev. during dev it takes in the access token. dev code is not allowed to be uploaded. if it is prod ready, then it asks for a name for this version and uploads it.

to check for prod readiness, when the space-window's code is pasted, it checks for `spacewindow-[32digitshexadecimal]` which is always the shape of the access token... and rejects that code from being used. this helps detect access token in comments which above method would fail to catch.

the iframe doesnt get to access any cookie from the parent. but it can render & call any cdn, or third party services it needs to show the view. keep a log of all external requests sent from the iframe and what data was sent & received.

Active Subscriptions:
Only when someone is active on a space window with a subscription, does the backend run the query and return it. it is client's responsibility to manage cursors in case of disconnected ws request.

Notifications:
These are similar to active subscritions but are always active & have the capability to send a "Notification" on the notifications modal in space station (present in the bottom of the left sidebar & notifications panel) via a notify(...) function.

it can be tested very simiarly to space-windows using access token and when an access token sends a notify, it only goes to the inbox of the person testing the notification.

Notifications can subscribe to records and process it internally and decide if it wants to trigger a notify, and with what text.