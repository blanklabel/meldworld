package main

import (
    "encoding/json"
    "html/template"
    "io/ioutil"
    "net/http"

    log "github.com/sirupsen/logrus"

    "github.com/blanklabel/meldworld/model"
    "github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{} // use default options
var homeTemplate = template.Must(template.ParseFiles("home.html"))

var gh = GameHub{
    clients:      make(map[string]*Player),
    broadcast:    make(chan *model.ClientMessage),
    register:     make(chan *Player),
    unregister:   make(chan Player),
    entityaction: make(chan *model.EntityAction),
    actionqueue:  make([]*model.EntityAction, 100), //buffer of 100 actions to kick us off
}

func auth(w http.ResponseWriter, r *http.Request) {
    // TODO: Auth mech
}

func game(w http.ResponseWriter, r *http.Request) {
    c, err := upgrader.Upgrade(w, r, nil)

    if err != nil {
        log.Warn("upgrade:", err)
        return
    }

    player := NewPlayer(c)
    gh.register <- player
    mHolder := &model.ModelType{}

    for {

        // Get Client message
        _, message, err := c.ReadMessage()
        if err != nil {
            log.Warn(err)
            gh.unregister <- *player
            break
        }

        json.Unmarshal(message, mHolder)

        log.Info(mHolder.MsgType)

        // Determine message type
        switch mHolder.MsgType {

        // Receive client messages
        case model.CLIENTMESSAGE:
            m := &model.ClientMessage{}
            json.Unmarshal(message, m)
            r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTMESSAGE},
                Msg:                             m.Msg, Sender: player.ID}

            gh.broadcast <- r

        case model.ENTITYACTION:
            m := &model.EntityAction{}
            json.Unmarshal(message, m)
            switch m.Action {
            case model.ENTITYACTIONMOVE:
                r := &model.EntityAction{
                    ModelType: model.ModelType{MsgType: model.ENTITYACTION},
                    Action:    model.ENTITYACTIONMOVE,
                    EntityMove: model.EntityMove{
                        Direction: m.Direction,
                        Distance:  m.Distance},
                }
                gh.entityaction <- r

            }

        default:
            log.Warn("Bad Message:", mHolder, message)
            r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTERROR},
                Msg:                             "Unknown Message Type", Sender: "Server"}
            gh.DirectMessage(r, player.ID)
        // fmt.Println(mHolder.MType)
        }
    }
}

func home(w http.ResponseWriter, r *http.Request) {
    homeTemplate.Execute(w, "ws://"+r.Host+"/game")
}

func main() {
    // TODO Command line
    file, e := ioutil.ReadFile("./example.json")

    if e != nil {
        log.Fatal("File error: %v\n", e)
    }

    world := &model.WorldMap{}
    json.Unmarshal(file, world)
    // fmt.Println(world)
    gh.WorldMapped = *world

    addr := "localhost:8080"
    // log.SetFlags(0)
    http.HandleFunc("/game", game)
    http.HandleFunc("/", home)
    http.HandleFunc("/auth", auth)

    //fmt.Println(world)

    //game loop
    go gh.ServeGame()

    // run web server
    log.Fatal(http.ListenAndServe(addr, nil))
}
