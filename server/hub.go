package main

import (
	"fmt"
	"time"

	"github.com/blanklabel/meldworld/model"
	"github.com/satori/go.uuid" // TODO REMOVE WHEN RECORD PULLED
	"github.com/sirupsen/logrus"
	log "github.com/sirupsen/logrus"
)

type GameHub struct {
	clients      map[string]*Player
	broadcast    chan *model.ClientMessage
	register     chan *Player
	unregister   chan Player
	entityaction chan *model.EntityAction
	actionqueue  []*model.EntityAction
	WorldMapped  model.WorldMap
}

// Add a new unknown client
func (g *GameHub) AddNewClient(p *Player) {
	// map the player to the hub
	g.clients[p.ID] = p

	// Notify everyone of the new player
	announcement := fmt.Sprintf("Player %s joined", p.ID)
	r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTJOIN},
		Msg: announcement, Sender: "Server"}
	g.Broadcast(r)

	// TODO: Pull new record
	gh.WorldMapped.Entities = append(gh.WorldMapped.Entities,
		model.Entity{
			ID:          uuid.NewV4().String(),
			OwnerID:     p.ID,
			Name:        "Bob",
			Full_hp:     20,
			C_hp:        20,
			Phy_def:     3,
			Phy_atk:     2,
			Speed:       1,
			Coordinates: model.Cords{X: 0, Y: 0},
			Destination: model.Cords{X: 0, Y: 0},
		})

	// Tell the player who they are
	g.DirectMessage(p.PlayerMeta, p.ID)

	// Give the new player the worldmap
	g.DirectMessage(g.WorldMapped, p.ID)

}

// Remove a client from the server
func (g *GameHub) RemoveClient(p Player) {
	// close the connection and delete the record
	g.clients[p.ID].conn.Close()
	delete(g.clients, p.ID)

	// Notify server of player departure
	announcement := fmt.Sprintf("Player %s left", p.ID)
	r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTLEAVE},
		Msg: announcement, Sender: "Server"}
	g.Broadcast(r)
}

// Write a message to a single client
func (g GameHub) DirectMessage(msg interface{}, userID string) {
	// TODO: Should track FROM & TO maybe?

	err := g.clients[userID].conn.WriteJSON(msg)
	if err != nil {
		log.Warn("Message Failed to send:", err, msg)
	}
}

// Account to all clients
func (g GameHub) Broadcast(msg *model.ClientMessage) {

	// loop through all clients and give them a message
	for id, client := range g.clients {
		err := client.conn.WriteJSON(msg)
		if err != nil {
			log.Warn("Didn't send due to error:", err)
		}
		log.WithFields(logrus.Fields{
			"messagetype": msg.MsgType,
			"content":     msg.Msg,
			"toplayer":    id,
		}).Info("Message sent")
	}
}

// Account to all clients
func (g GameHub) ActionBroadcast(msg *model.EntityAction) {

	// loop through all clients and give them a message
	for id, client := range g.clients {
		err := client.conn.WriteJSON(msg)
		if err != nil {
			log.Warn("Didn't send due to error:", err)
		}
		log.Info("ACTION:", msg.MsgType, id)
	}
}

func (g GameHub) EntityAction(action *model.EntityAction) {
	switch action.Action {
	case model.ENTITYACTIONMOVE:
		log.WithFields(logrus.Fields{
			"actiontype": action.Action,
			"direction":  action.Direction,
			"distance":   action.Distance,
		}).Info("ACTION")
		// TODO: if the move command is valid
		gh.actionqueue = append(gh.actionqueue, action)
		break
	}
}

func (g GameHub) ServeGame() {

	//running at 60 FPS everyone get the ticks  -- int(1e9) / 60
	// Once a second everyone gets an update
	frameNS := time.Duration(time.Second)
	clk := time.NewTicker(frameNS)

	// Run forever
	for {
		select {
		// main loop
		case <-clk.C:
			//for _, e := range gh.WorldMapped.Entities {
			//	gh.Broadcast(&model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTMESSAGE},
			//		Msg:    fmt.Sprintf("{\"LOCATION\": {X: %d, Y: %d}}", e.Coordinates.X, e.Coordinates.X),
			//		Sender: "Server"})
			//}
			for _, action := range gh.actionqueue {
				if action != nil {
					fmt.Println("ACTION", action)
				}
			}
			// TODO: Update world map with new activity
			// Go through a queue of actions
			// if location isn't a collision announce destination
			// if is valid (Speed + tile direction) allow move
			// else return error to client

			// announce new locations
		case c := <-gh.register:
			gh.AddNewClient(c)
		case c := <-gh.unregister:
			gh.RemoveClient(c)
		case c := <-gh.broadcast:
			gh.Broadcast(c)
		case c := <-gh.entityaction:
			gh.EntityAction(c)
		}
	}
}
